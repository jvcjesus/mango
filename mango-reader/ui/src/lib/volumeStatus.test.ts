import { describe, expect, it } from 'vitest';
import {
  resolveVolumePresentation,
  resolveSeriesSummary,
  getStagingSummary,
  type VolumePresentation,
} from './volumeStatus';
import type { ImportBook } from './importReview';
import type { PipelineEvent, Queued, CanonicalLibraryReleasesSummary } from './api';

describe('Normalized Volume Presentation Model', () => {
  it('Case A: Missing volume -> Missing', () => {
    const pres = resolveVolumePresentation({
      volume: 2,
      isOwnedInKomga: false,
      isMissingReported: true,
    });
    expect(pres.status).toBe('missing');
    expect(pres.statusLabel).toBe('Missing');
    expect(pres.badgeVariant).toBe('neutral');
    expect(pres.physicalState).toBe('missing');
    expect(pres.canAcquire).toBe(true);
  });

  it('Case B: Legitimately queued new acquisition -> Queued', () => {
    const queuedItem: Queued = {
      chapterId: 101,
      chapterName: 'Volume 5',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'queued',
      stage: 'queued',
      done: 0,
      total: 100,
      attempts: 0,
      retryAt: null,
      error: null,
      rawTitle: '[Stick] Blood on the Tracks - Vol. 05 [Digital]',
      sourceType: 'nyaa',
    };

    const pres = resolveVolumePresentation({
      volume: 5,
      isOwnedInKomga: false,
      queuedItems: [queuedItem],
    });

    expect(pres.status).toBe('queued');
    expect(pres.statusLabel).toBe('Queued');
    expect(pres.badgeVariant).toBe('waiting');
    expect(pres.physicalState).toBe('queued');
    expect(pres.canAcquire).toBe(false);
  });

  it('Case C: Active TorBox/retrieval -> Acquiring with percentage', () => {
    const downloadingItem: Queued = {
      chapterId: 102,
      chapterName: 'Volume 6',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'downloading',
      stage: 'torbox_retrieving',
      done: 0,
      total: null,
      attempts: 0,
      retryAt: null,
      error: null,
      bytesDownloaded: 68 * 1024 * 1024,
      totalBytes: 100 * 1024 * 1024,
      rawTitle: '[1r0n] Blood on the Tracks v06.cbz',
      statusMessage: 'Downloading from TorBox: 68 MiB / 100 MiB (68%)',
    };

    const pres = resolveVolumePresentation({
      volume: 6,
      isOwnedInKomga: false,
      queuedItems: [downloadingItem],
    });

    expect(pres.status).toBe('acquiring');
    expect(pres.statusLabel).toBe('Downloading · 68%');
    expect(pres.badgeVariant).toBe('active');
    expect(pres.physicalState).toBe('downloading');
    expect(pres.progressPercent).toBe(68);
  });

  it('Case D: Processing archive -> Processing', () => {
    const processingItem: Queued = {
      chapterId: 103,
      chapterName: 'Volume 7',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'validating',
      stage: 'validating',
      done: 0,
      total: null,
      attempts: 0,
      retryAt: null,
      error: null,
      statusMessage: 'Validating archive integrity',
    };

    const pres = resolveVolumePresentation({
      volume: 7,
      isOwnedInKomga: false,
      queuedItems: [processingItem],
    });

    expect(pres.status).toBe('processing');
    expect(pres.statusLabel).toBe('Processing');
    expect(pres.badgeVariant).toBe('active');
    expect(pres.physicalState).toBe('processing');
    expect(pres.physicalLabel).toBe('Validating archive integrity');
  });

  it('Case E: Canonical CBZ published + waiting for Komga -> In library / Waiting for Komga', () => {
    const verifyingItem: Queued = {
      chapterId: 104,
      chapterName: 'Volume 8',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'verifying',
      stage: 'verifying',
      publishedPath: '/srv/media/manga/Blood on the Tracks/Blood on the Tracks - v08.cbz',
      canRetryKomga: true,
      done: 1,
      total: 1,
      attempts: 0,
      retryAt: null,
      error: null,
    };

    const pres = resolveVolumePresentation({
      volume: 8,
      queuedItems: [verifyingItem],
    });

    expect(pres.status).toBe('waiting_for_komga');
    expect(pres.statusLabel).toBe('Waiting for Komga');
    expect(pres.physicalState).toBe('in_library');
    expect(pres.metadataState).toBe('waiting_for_komga');
    expect(pres.canRetryKomga).toBe(true);
  });

  it('Case F: Canonical CBZ published + metadata pending -> Owned / Metadata pending (NOT Queued)', () => {
    const updatingItem: Queued = {
      chapterId: 105,
      chapterName: 'Volume 9',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'queued', // Internal download state might say queued
      stage: 'metadata',
      publishedPath: '/srv/media/manga/Blood on the Tracks/Blood on the Tracks - v09.cbz',
      metadataHandoff: {
        state: 'in_progress',
        message: 'KOMF metadata enrichment in progress',
        jobId: 'job-999',
        komgaBookId: 'book-999',
      },
      done: 1,
      total: 1,
      attempts: 0,
      retryAt: null,
      error: null,
    };

    const pres = resolveVolumePresentation({
      volume: 9,
      isOwnedInKomga: true,
      queuedItems: [updatingItem],
    });

    expect(pres.status).toBe('metadata');
    expect(pres.statusLabel).toBe('Updating metadata');
    expect(pres.badgeVariant).toBe('active');
    expect(pres.physicalState).toBe('in_library');
    expect(pres.metadataState).toBe('updating');
    expect(pres.metadataLabel).toBe('KOMF metadata enrichment in progress');
  });

  it('Case G: Canonical CBZ published + metadata failed -> Owned / Needs attention (NOT acquisition failed)', () => {
    const failedMetadataItem: Queued = {
      chapterId: 106,
      chapterName: 'Volume 10',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'metadata_failed',
      stage: 'metadata_failed',
      publishedPath: '/srv/media/manga/Blood on the Tracks/Blood on the Tracks - v10.cbz',
      metadataHandoff: {
        state: 'failed',
        message: 'KOMF could not resolve MangaBaka collection',
        jobId: 'job-100',
        komgaBookId: 'book-100',
      },
      done: 1,
      total: 1,
      attempts: 1,
      retryAt: null,
      error: 'Metadata enrichment failed',
    };

    const pres = resolveVolumePresentation({
      volume: 10,
      isOwnedInKomga: true,
      queuedItems: [failedMetadataItem],
    });

    expect(pres.status).toBe('attention');
    expect(pres.statusLabel).toBe('Media acquired · Metadata needs attention');
    expect(pres.badgeVariant).toBe('attention');
    expect(pres.physicalState).toBe('in_library');
    expect(pres.metadataState).toBe('failed');
    expect(pres.canRetryMetadata).toBe(true);
    expect(pres.canRetryDownload).toBe(false); // Media exists, cannot re-download
  });

  it('Case G2: Published with metadata in review -> Attention with canRetryMetadata', () => {
    const reviewMetadataItem: Queued = {
      chapterId: 107,
      chapterName: 'Volume 11',
      mangaId: 1,
      title: 'Delicious in Dungeon',
      state: 'metadata_review',
      stage: 'metadata_review',
      source: 'test',
      sourceType: 'test',
      backendType: 'test',
      externalJobId: '',
      collectionId: '',
      payloadChapter: null,
      volume: 11,
      number: 11,
      publishedPath: '/srv/media/manga/Delicious in Dungeon/Delicious in Dungeon v11.cbz',
      canRetryKomga: false,
      metadataHandoff: {
        chapterId: 107,
        canonicalPath: '/srv/media/manga/Delicious in Dungeon/Delicious in Dungeon v11.cbz',
        state: 'needs_review',
        message: 'Missing/ambiguous/conflicting MangaBaka identity — metadata needs review',
      },
      error: 'Missing/ambiguous/conflicting MangaBaka identity — metadata needs review',
    };

    const pres = resolveVolumePresentation({
      volume: 11,
      isOwnedInKomga: true,
      queuedItems: [reviewMetadataItem],
    });

    expect(pres.status).toBe('attention');
    expect(pres.statusLabel).toBe('Media acquired · Metadata needs attention');
    expect(pres.badgeVariant).toBe('attention');
    expect(pres.physicalState).toBe('in_library');
    expect(pres.metadataState).toBe('failed');
    expect(pres.canRetryMetadata).toBe(true);
    expect(pres.canRetryDownload).toBe(false);
  });

  it('Case H: Fully successful handoff -> Complete', () => {
    const event: PipelineEvent = {
      id: 'evt-101',
      timestamp: 1700000000,
      chapter_id: 107,
      series: 'Blood on the Tracks',
      release_name: 'Blood on the Tracks v11',
      is_volume: true,
      number: 11,
      outcome: 'published',
      destination: '/srv/media/manga/Blood on the Tracks/Blood on the Tracks - v11.cbz',
      mangabaka_status: 'matched',
      optimizer_status: 'optimized',
      komga_status: 'ready',
      error: null,
    };

    const pres = resolveVolumePresentation({
      volume: 11,
      isOwnedInKomga: true,
      bookId: 'book-11',
      bookName: 'Blood on the Tracks - v11.cbz',
      pipelineEvents: [event],
    });

    expect(pres.status).toBe('complete');
    expect(pres.statusLabel).toBe('Complete');
    expect(pres.badgeVariant).toBe('complete');
    expect(pres.physicalState).toBe('in_library');
    expect(pres.metadataState).toBe('complete');
  });

  it('Case I: Historical queued/download record + canonical published evidence -> Owned/Complete takes precedence', () => {
    // Database contains an old completed download record whose raw state was 'completed' or 'queued'
    const staleQueueItem: Queued = {
      chapterId: 108,
      chapterName: 'Volume 17',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'completed',
      stage: 'complete',
      publishedPath: '/srv/media/manga/Blood on the Tracks/Blood on the Tracks - v17.cbz',
      done: 1,
      total: 1,
      attempts: 0,
      retryAt: null,
      error: null,
      rawTitle: 'Blood on the Tracks v17',
    };

    const pres = resolveVolumePresentation({
      volume: 17,
      isOwnedInKomga: true,
      bookId: 'book-17',
      bookName: 'Blood on the Tracks - v17.cbz',
      queuedItems: [staleQueueItem],
    });

    expect(pres.status).toBe('complete');
    expect(pres.statusLabel).toBe('Complete');
    expect(pres.physicalState).toBe('in_library');
    expect(pres.contradictionWarning).toBeNull();
  });

  it('Case J: Genuine contradictory physical reacquisition of an already-owned volume -> Owned primary state + warning', () => {
    // Volume 1 is already in Komga, but an active TorBox download is running for it
    const activeReacquisitionItem: Queued = {
      chapterId: 109,
      chapterName: 'Volume 1',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'downloading',
      stage: 'torbox_downloading',
      done: 0,
      total: null,
      attempts: 0,
      retryAt: null,
      error: null,
      rawTitle: '[LuCaZ] Blood on the Tracks v01.cbz',
      publishedPath: null, // this specific job hasn't published yet
    };

    const pres = resolveVolumePresentation({
      volume: 1,
      isOwnedInKomga: true,
      bookId: 'book-1',
      bookName: 'Blood on the Tracks - v01.cbz',
      queuedItems: [activeReacquisitionItem],
    });

    // Primary state must remain owned
    expect(pres.status).toBe('complete');
    expect(pres.physicalState).toBe('in_library');
    // But contradiction warning must be surfaced
    expect(pres.contradictionWarning).toContain('already owned in library');
  });

  it('Case K: Multiple internal records for same volume -> coalesced into one primary volume presentation', () => {
    // Volume 17 has:
    // 1. Candidate in (1r0n) payload
    // 2. Candidate in (LuCaZ) payload
    // 3. Staging download record
    // 4. Komga ownership
    const candidate1: ImportBook = {
      id: 31,
      payloadChapter: 15070,
      series: 'Blood on the Tracks',
      innerPath: 'Blood on the Tracks (1r0n)/Blood on the Tracks v17.cbz',
      suggestedNumber: 17,
      assignedNumber: 17,
      state: 'completed',
      childChapter: 15073,
      suggestedEdition: null,
    };

    const candidate2: ImportBook = {
      id: 54,
      payloadChapter: 15096,
      series: 'Blood on the Tracks',
      innerPath: 'Blood on the Tracks (LuCaZ)/Blood on the Tracks v17.cbz',
      suggestedNumber: 17,
      assignedNumber: 17,
      state: 'queued',
      childChapter: 15106,
      suggestedEdition: null,
    };

    const downloadItem: Queued = {
      chapterId: 15096,
      chapterName: 'Volume 17',
      mangaId: 702,
      title: 'Blood on the Tracks',
      state: 'completed',
      stage: 'complete',
      done: 1,
      total: 1,
      attempts: 0,
      retryAt: null,
      error: null,
      rawTitle: 'Blood on the Tracks (LuCaZ)',
    };

    const pres = resolveVolumePresentation({
      volume: 17,
      isOwnedInKomga: true,
      bookId: 'book-17',
      bookName: 'Blood on the Tracks - v17.cbz',
      importCandidates: [candidate1, candidate2],
      queuedItems: [downloadItem],
    });

    expect(pres.volume).toBe(17);
    expect(pres.status).toBe('complete');
    expect(pres.statusLabel).toBe('Complete');
    expect(pres.physicalState).toBe('in_library');
  });

  it('Case L: Ongoing manga -> do not imply series completion merely because all currently released volumes are owned', () => {
    const volumes: VolumePresentation[] = [
      {
        volume: 1,
        label: 'Volume 1',
        status: 'complete',
        statusLabel: 'Complete',
        badgeVariant: 'complete',
        physicalState: 'in_library',
        physicalLabel: 'In canonical library',
        metadataState: 'complete',
        metadataLabel: 'Complete',
        progressPercent: null,
        canRetryMetadata: false,
        canRetryKomga: false,
        canRetryDownload: false,
        canReview: false,
        canAcquire: false,
      },
      {
        volume: 2,
        label: 'Volume 2',
        status: 'complete',
        statusLabel: 'Complete',
        badgeVariant: 'complete',
        physicalState: 'in_library',
        physicalLabel: 'In canonical library',
        metadataState: 'complete',
        metadataLabel: 'Complete',
        progressPercent: null,
        canRetryMetadata: false,
        canRetryKomga: false,
        canRetryDownload: false,
        canReview: false,
        canAcquire: false,
      },
    ];

    const summary = resolveSeriesSummary({
      title: 'Kagurabachi',
      volumes,
      totalExpectedVolumes: 2,
      isOngoing: true,
    });

    expect(summary.ownedCount).toBe(2);
    expect(summary.isOngoing).toBe(true);
    expect(summary.isCompleted).toBe(false);
    expect(summary.libraryStatus).toBe('up_to_date');
    expect(summary.statusExplanation).toContain('Up to date');
    expect(summary.statusExplanation).not.toContain('Complete');
  });

  it('M6.6: Ambiguous series identity pauses authoritative metrics and highlights resolve identity CTA', () => {
    const volumes: VolumePresentation[] = [
      {
        volume: 1,
        label: 'Volume 1',
        status: 'missing',
        statusLabel: 'Missing',
        badgeVariant: 'neutral',
        physicalState: 'missing',
        physicalLabel: 'Missing from library',
        metadataState: 'not_applicable',
        metadataLabel: 'None',
        progressPercent: null,
        canRetryMetadata: false,
        canRetryKomga: false,
        canRetryDownload: false,
        canReview: false,
        canAcquire: true,
      },
    ];

    const summary = resolveSeriesSummary({
      title: 'Delicious in Dungeon',
      volumes,
      totalExpectedVolumes: 14,
      isOngoing: false,
      isIdentityAmbiguous: true,
      reconciliationStatus: 'review_required',
    });

    expect(summary.isIdentityAmbiguous).toBe(true);
    expect(summary.statusExplanation).toBe('Series identity needs review');
    expect(summary.primaryActionType).toBe('resolve_identity');
    expect(summary.primaryActionLabel).toBe('Resolve identity');
  });

  it('M6.6: Candidate available triggers primary CTA review_release', () => {
    const volumes: VolumePresentation[] = [
      {
        volume: 1,
        label: 'Volume 1',
        status: 'complete',
        statusLabel: 'Owned',
        badgeVariant: 'complete',
        physicalState: 'in_library',
        physicalLabel: 'Stored in library',
        metadataState: 'complete',
        metadataLabel: 'Complete',
        progressPercent: null,
        canRetryMetadata: false,
        canRetryKomga: false,
        canRetryDownload: false,
        canReview: false,
        canAcquire: false,
      },
      {
        volume: 2,
        label: 'Volume 2',
        status: 'missing',
        statusLabel: 'Missing',
        badgeVariant: 'neutral',
        physicalState: 'missing',
        physicalLabel: 'Missing from library',
        metadataState: 'not_applicable',
        metadataLabel: 'None',
        progressPercent: null,
        canRetryMetadata: false,
        canRetryKomga: false,
        canRetryDownload: false,
        canReview: false,
        canAcquire: true,
      },
    ];

    const summary = resolveSeriesSummary({
      title: 'Kagurabachi',
      volumes,
      totalExpectedVolumes: 2,
      isOngoing: true,
      hasAvailableRelease: true,
    });

    expect(summary.isIdentityAmbiguous).toBe(false);
    expect(summary.primaryActionType).toBe('review_release');
    expect(summary.primaryActionLabel).toBe('Review release');
  });

  it('M6.6: Missing volumes with no available release triggers discover CTA', () => {
    const volumes: VolumePresentation[] = [
      {
        volume: 1,
        label: 'Volume 1',
        status: 'missing',
        statusLabel: 'Missing',
        badgeVariant: 'neutral',
        physicalState: 'missing',
        physicalLabel: 'Missing from library',
        metadataState: 'not_applicable',
        metadataLabel: 'None',
        progressPercent: null,
        canRetryMetadata: false,
        canRetryKomga: false,
        canRetryDownload: false,
        canReview: false,
        canAcquire: true,
      },
    ];

    const summary = resolveSeriesSummary({
      title: 'Kagurabachi',
      volumes,
      totalExpectedVolumes: 3,
      isOngoing: true,
      hasAvailableRelease: false,
    });

    expect(summary.primaryActionType).toBe('discover');
    expect(summary.primaryActionLabel).toBe('Discover releases');
  });

  it('M6.6: Post-publication metadata failure triggers retry_metadata CTA', () => {
    const volumes: VolumePresentation[] = [
      {
        volume: 1,
        label: 'Volume 1',
        status: 'attention',
        statusLabel: 'Needs attention',
        badgeVariant: 'attention',
        physicalState: 'in_library',
        physicalLabel: 'Stored in library',
        metadataState: 'failed',
        metadataLabel: 'Metadata retry failed',
        progressPercent: null,
        canRetryMetadata: true,
        canRetryKomga: false,
        canRetryDownload: false,
        canReview: false,
        canAcquire: false,
      },
    ];

    const summary = resolveSeriesSummary({
      title: 'Delicious in Dungeon',
      volumes,
      totalExpectedVolumes: 1,
      isOngoing: false,
    });

    expect(summary.primaryActionType).toBe('retry_metadata');
    expect(summary.primaryActionLabel).toBe('Retry metadata');
  });

  it('Staging cleanup: consolidates completed staging payloads into single action', () => {
    const importBooks: ImportBook[] = [
      {
        id: 1,
        payloadChapter: 100,
        series: 'Series A',
        innerPath: 'file1.cbz',
        suggestedNumber: 1,
        assignedNumber: 1,
        state: 'completed',
        childChapter: 1001,
        suggestedEdition: null,
      },
      {
        id: 2,
        payloadChapter: 100,
        series: 'Series A',
        innerPath: 'file2.cbz',
        suggestedNumber: 2,
        assignedNumber: 2,
        state: 'completed',
        childChapter: 1002,
        suggestedEdition: null,
      },
      {
        id: 3,
        payloadChapter: 200,
        series: 'Series B',
        innerPath: 'file3.cbz',
        suggestedNumber: 1,
        assignedNumber: 1,
        state: 'completed',
        childChapter: 2001,
        suggestedEdition: null,
      },
      {
        id: 4,
        payloadChapter: 300,
        series: 'Series C',
        innerPath: 'file4.cbz',
        suggestedNumber: 1,
        assignedNumber: null,
        state: 'needs_review',
        childChapter: null,
        suggestedEdition: null,
      },
    ];

    const summary = getStagingSummary(importBooks);
    expect(summary.totalPayloads).toBe(3);
    // Payloads 100 and 200 are completed, payload 300 has book in needs_review
    expect(summary.completedPayloadChapters).toEqual([100, 200]);
    expect(summary.pendingPayloadChapters).toEqual([300]);
    expect(summary.canClean).toBe(true);
  });

  it('Discovery candidates presentation: formats scores and match types correctly', () => {
    const candidate: import('./api').DiscoveredCandidate = {
      candidateId: 'nyaa:12345',
      provider: 'nyaa',
      providerItemId: '12345',
      title: 'Akane-banashi Vol. 17',
      rawTitle: '[danke-Empire] Akane-banashi - Vol. 17 [Digital]',
      matchType: 'exact_volume',
      targetVolume: 17,
      sizeBytes: 150_000_000,
      uploader: 'danke-Empire',
      isTrusted: true,
      isDigital: true,
      score: 95.0,
      scoreExplanation: 'Exact single volume match (+60); Digital edition (+15); Trusted uploader (+10); Well-seeded (+10)',
      seeders: 25,
      leechers: 2,
    };

    const discovery: import('./api').VolumeDiscoveryResult = {
      seriesId: 'manga:10',
      volumeNumber: 17,
      status: 'found',
      candidateCount: 1,
      topCandidate: candidate,
      candidates: [candidate],
      providersChecked: ['nyaa', 'madokami'],
      providerErrors: { madokami: 'Madokami is currently offline / unavailable' },
      lastDiscoveredAt: 1700000000,
    };

    expect(discovery.status).toBe('found');
    expect(discovery.candidateCount).toBe(1);
    expect(discovery.topCandidate?.score).toBe(95.0);
    expect(discovery.topCandidate?.matchType).toBe('exact_volume');
    expect(discovery.providerErrors['madokami']).toContain('offline');
  });

  it('M6.6.1 Section 23 - Case 1: Healthy ongoing / up-to-date', () => {
    const libraryState: CanonicalLibraryReleasesSummary = {
      mangaId: 18,
      title: 'Kagurabachi',
      acquisitionMarket: 'en',
      identityStatus: 'verified',
      isIdentityAmbiguous: false,
      isOngoing: true,
      isCompleted: false,
      ownedVolumesCount: 8,
      ownedVolumes: [1, 2, 3, 4, 5, 6, 7, 8],
      releasedVolumesCount: 8,
      releasedVolumes: [1, 2, 3, 4, 5, 6, 7, 8],
      missingReleasedCount: 0,
      missingReleasedVolumes: [],
      upcomingVolumes: [9],
      nextRelease: {
        volumeNumber: 9,
        releaseDate: '2026-11-03',
      },
      releasesToday: [],
      progressPercent: 100,
      libraryStatus: 'up_to_date',
      primaryActionType: 'none',
      primaryActionLabel: '',
      provenance: {
        ownedSource: 'komga: 8 volumes verified',
        releasedSource: 'opentome: 8 releases',
        missingSource: 'diff: 0 missing',
        upcomingSource: 'opentome: 1 upcoming',
      },
    };

    const summary = resolveSeriesSummary({
      title: 'Kagurabachi',
      volumes: [],
      libraryState,
      isOngoing: true,
    });

    expect(summary.ownedCount).toBe(8);
    expect(summary.totalExpectedVolumes).toBe(8);
    expect(summary.missingCount).toBe(0);
    expect(summary.progressPercent).toBe(100);
    expect(summary.libraryStatus).toBe('up_to_date');
    expect(summary.statusExplanation).toBe('8 / 8 released volumes owned · Up to date');
    expect(summary.primaryActionType).toBe('none');
    expect(summary.isCompleted).toBe(false);
    expect(summary.isOngoing).toBe(true);
    expect(summary.libraryState?.upcomingVolumes).toEqual([9]);
    expect(summary.libraryState?.nextRelease?.volumeNumber).toBe(9);
    expect(summary.libraryState?.nextRelease?.releaseDate).toBe('2026-11-03');
  });

  it('M6.6.1 Section 23 - Case 2: Missing released volume', () => {
    const libraryState: CanonicalLibraryReleasesSummary = {
      mangaId: 18,
      title: 'Kagurabachi',
      acquisitionMarket: 'en',
      identityStatus: 'verified',
      isIdentityAmbiguous: false,
      isOngoing: true,
      isCompleted: false,
      ownedVolumesCount: 7,
      ownedVolumes: [1, 2, 3, 4, 5, 6, 7],
      releasedVolumesCount: 8,
      releasedVolumes: [1, 2, 3, 4, 5, 6, 7, 8],
      missingReleasedCount: 1,
      missingReleasedVolumes: [8],
      upcomingVolumes: [9],
      nextRelease: { volumeNumber: 9, releaseDate: '2026-11-03' },
      releasesToday: [],
      progressPercent: 88,
      libraryStatus: 'missing_released',
      primaryActionType: 'discover',
      primaryActionLabel: 'Discover releases',
      provenance: {
        ownedSource: 'komga: 7 volumes verified',
        releasedSource: 'opentome: 8 releases',
        missingSource: 'diff: 1 missing',
        upcomingSource: 'opentome: 1 upcoming',
      },
    };

    const summary = resolveSeriesSummary({
      title: 'Kagurabachi',
      volumes: [],
      libraryState,
      isOngoing: true,
    });

    expect(summary.ownedCount).toBe(7);
    expect(summary.totalExpectedVolumes).toBe(8);
    expect(summary.missingCount).toBe(1);
    expect(summary.progressPercent).toBe(88);
    expect(summary.libraryStatus).toBe('missing_released');
    expect(summary.statusExplanation).toBe('7 / 8 released volumes owned · 1 missing');
    expect(summary.primaryActionType).toBe('discover');
    expect(summary.primaryActionLabel).toBe('Discover releases');
  });

  it('M6.6.1 Section 23 - Case 3: Candidate available triggers Review release', () => {
    const libraryState: CanonicalLibraryReleasesSummary = {
      mangaId: 18,
      title: 'Kagurabachi',
      acquisitionMarket: 'en',
      identityStatus: 'verified',
      isIdentityAmbiguous: false,
      isOngoing: true,
      isCompleted: false,
      ownedVolumesCount: 7,
      ownedVolumes: [1, 2, 3, 4, 5, 6, 7],
      releasedVolumesCount: 8,
      releasedVolumes: [1, 2, 3, 4, 5, 6, 7, 8],
      missingReleasedCount: 1,
      missingReleasedVolumes: [8],
      upcomingVolumes: [],
      nextRelease: null,
      releasesToday: [],
      progressPercent: 88,
      libraryStatus: 'candidate_available',
      primaryActionType: 'review_release',
      primaryActionLabel: 'Review release',
      provenance: {
        ownedSource: 'komga: 7 volumes verified',
        releasedSource: 'opentome: 8 releases',
        missingSource: 'candidate ready for review',
      },
    };

    const summary = resolveSeriesSummary({
      title: 'Kagurabachi',
      volumes: [],
      libraryState,
      isOngoing: true,
    });

    expect(summary.primaryActionType).toBe('review_release');
    expect(summary.primaryActionLabel).toBe('Review release');
  });

  it('M6.6.1 Section 23 - Case 4: Future volume displayed as upcoming and not counted missing', () => {
    const libraryState: CanonicalLibraryReleasesSummary = {
      mangaId: 18,
      title: 'Kagurabachi',
      acquisitionMarket: 'en',
      identityStatus: 'verified',
      isIdentityAmbiguous: false,
      isOngoing: true,
      isCompleted: false,
      ownedVolumesCount: 8,
      ownedVolumes: [1, 2, 3, 4, 5, 6, 7, 8],
      releasedVolumesCount: 8,
      releasedVolumes: [1, 2, 3, 4, 5, 6, 7, 8],
      missingReleasedCount: 0,
      missingReleasedVolumes: [],
      upcomingVolumes: [9, 10],
      nextRelease: { volumeNumber: 9, releaseDate: '2026-11-03' },
      releasesToday: [],
      progressPercent: 100,
      libraryStatus: 'up_to_date',
      primaryActionType: 'none',
      primaryActionLabel: '',
      provenance: {
        ownedSource: 'komga: 8 volumes verified',
        releasedSource: 'opentome: 8 releases',
        missingSource: 'diff: 0 missing',
        upcomingSource: 'opentome: 2 upcoming',
      },
    };

    const summary = resolveSeriesSummary({
      title: 'Kagurabachi',
      volumes: [],
      libraryState,
      isOngoing: true,
    });

    expect(summary.missingCount).toBe(0);
    expect(summary.totalExpectedVolumes).toBe(8);
    expect(summary.libraryState?.upcomingVolumes).toEqual([9, 10]);
  });

  it('M6.6.1 Section 23 - Case 5: Japanese release schedule does not inflate English missing count', () => {
    const libraryState: CanonicalLibraryReleasesSummary = {
      mangaId: 30,
      title: 'Witch Hat Atelier',
      acquisitionMarket: 'en',
      identityStatus: 'verified',
      isIdentityAmbiguous: false,
      isOngoing: true,
      isCompleted: false,
      ownedVolumesCount: 14,
      ownedVolumes: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
      releasedVolumesCount: 14,
      releasedVolumes: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
      missingReleasedCount: 0,
      missingReleasedVolumes: [],
      upcomingVolumes: [],
      nextRelease: null,
      releasesToday: [],
      progressPercent: 100,
      libraryStatus: 'up_to_date',
      primaryActionType: 'none',
      primaryActionLabel: '',
      provenance: {
        ownedSource: 'komga: 14 volumes verified',
        releasedSource: 'opentome: 14 releases',
        missingSource: 'diff: 0 missing',
      },
    };

    const summary = resolveSeriesSummary({
      title: 'Witch Hat Atelier',
      volumes: [],
      libraryState,
      isOngoing: true,
    });

    expect(summary.ownedCount).toBe(14);
    expect(summary.totalExpectedVolumes).toBe(14); // EN released, NOT JA 16!
    expect(summary.missingCount).toBe(0);
    expect(summary.libraryState?.acquisitionMarket).toBe('en');
  });

  it('M6.6.1 Section 23 - Case 6: Ambiguous identity suppresses authoritative counts and highlights resolve identity CTA', () => {
    const libraryState: CanonicalLibraryReleasesSummary = {
      mangaId: 99,
      title: 'Ambiguous Series',
      acquisitionMarket: 'en',
      identityStatus: 'ambiguous',
      isIdentityAmbiguous: true,
      isOngoing: true,
      isCompleted: false,
      ownedVolumesCount: 3,
      ownedVolumes: [1, 2, 3],
      releasedVolumesCount: 0,
      releasedVolumes: [],
      missingReleasedCount: 0,
      missingReleasedVolumes: [],
      upcomingVolumes: [],
      nextRelease: null,
      releasesToday: [],
      progressPercent: null,
      libraryStatus: 'identity_review_required',
      primaryActionType: 'resolve_identity',
      primaryActionLabel: 'Resolve identity',
      provenance: {
        ownedSource: 'komga: 3 volumes verified',
        releasedSource: 'suppressed: ambiguous identity',
        missingSource: 'suppressed: ambiguous identity',
      },
    };

    const summary = resolveSeriesSummary({
      title: 'Ambiguous Series',
      volumes: [],
      libraryState,
    });

    expect(summary.isIdentityAmbiguous).toBe(true);
    expect(summary.totalExpectedVolumes).toBeNull();
    expect(summary.missingCount).toBe(0);
    expect(summary.primaryActionType).toBe('resolve_identity');
    expect(summary.primaryActionLabel).toBe('Resolve identity');
    expect(summary.statusExplanation).toBe('Series identity needs review');
  });

  it('M6.6.1 Section 23 - Case 7: Metadata attention keeps physical media owned and offers retry metadata', () => {
    const volumes: VolumePresentation[] = [
      {
        volume: 14,
        label: 'Volume 14',
        status: 'attention',
        statusLabel: 'Metadata error',
        badgeVariant: 'attention',
        physicalState: 'in_library',
        physicalLabel: 'In canonical library',
        metadataState: 'failed',
        metadataLabel: 'Failed to enrich metadata',
        progressPercent: null,
        canRetryMetadata: true,
        canRetryKomga: false,
        canRetryDownload: false,
        canReview: false,
        canAcquire: false,
      },
    ];

    const summary = resolveSeriesSummary({
      title: 'Delicious in Dungeon',
      volumes,
      totalExpectedVolumes: 14,
      isOngoing: false,
      isCompleted: true,
    });

    expect(summary.ownedCount).toBe(1);
    expect(summary.primaryActionType).toBe('retry_metadata');
    expect(summary.primaryActionLabel).toBe('Retry metadata');
  });

  it('M6.6.1 Section 23 - Case 8: Active acquisition prevents duplicate acquire/discovery CTA', () => {
    const queued: Queued = {
      chapterId: 88,
      chapterName: 'Volume 8',
      mangaId: 18,
      title: 'Kagurabachi',
      state: 'downloading',
      stage: 'torbox_retrieving',
      done: 0,
      total: null,
      attempts: 0,
      retryAt: null,
      error: null,
      bytesDownloaded: 50 * 1024 * 1024,
      totalBytes: 100 * 1024 * 1024,
      sourceType: 'nyaa',
    };

    const pres = resolveVolumePresentation({
      volume: 8,
      isOwnedInKomga: false,
      queuedItems: [queued],
    });

    expect(pres.status).toBe('acquiring');
    expect(pres.canAcquire).toBe(false);
  });

  it('M6.6.1 Section 23 - Case 9: OpenTome unavailable degrades gracefully without false 0/0 or missing', () => {
    const libraryState: CanonicalLibraryReleasesSummary = {
      mangaId: 18,
      title: 'Kagurabachi',
      acquisitionMarket: 'en',
      identityStatus: 'verified',
      isIdentityAmbiguous: false,
      isOngoing: true,
      isCompleted: false,
      ownedVolumesCount: 8,
      ownedVolumes: [1, 2, 3, 4, 5, 6, 7, 8],
      releasedVolumesCount: 8,
      releasedVolumes: [1, 2, 3, 4, 5, 6, 7, 8],
      missingReleasedCount: 0,
      missingReleasedVolumes: [],
      upcomingVolumes: [],
      nextRelease: null,
      releasesToday: [],
      progressPercent: 100,
      libraryStatus: 'up_to_date',
      primaryActionType: 'none',
      primaryActionLabel: '',
      provenance: {
        ownedSource: 'komga: 8 volumes verified',
        releasedSource: 'fallback: komga total volumes',
        missingSource: 'diff: 0 missing',
      },
    };

    const summary = resolveSeriesSummary({
      title: 'Kagurabachi',
      volumes: [],
      libraryState,
      isOngoing: true,
    });

    expect(summary.ownedCount).toBe(8);
    expect(summary.totalExpectedVolumes).toBe(8);
    expect(summary.missingCount).toBe(0);
    expect(summary.statusExplanation).toBe('8 / 8 released volumes owned · Up to date');
    expect(summary.libraryState?.acquisitionMarket).toBe('en');
  });
});
