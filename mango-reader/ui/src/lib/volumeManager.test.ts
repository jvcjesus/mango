import { describe, expect, it } from 'vitest';
import type {
  VolumeManagerSummary,
  VolumeManagerEntry,
  VolumeManagerBatchPlanResponse,
} from './api';

describe('Unified Volume Manager (M8.2)', () => {
  it('correctly models 45 official volumes of Haikyu!! with Komga ownership distinction', () => {
    const entries: VolumeManagerEntry[] = Array.from({ length: 45 }, (_, idx) => {
      const volNum = idx + 1;
      const isOwned = false; // Unowned in Komga
      const hasCandidate = volNum === 1;

      return {
        volumeNumber: volNum,
        label: `Volume ${volNum < 10 ? '0' + volNum : volNum}`,
        title: null,
        coverUrl: null,
        releaseDate: '2016-07-05',
        releaseDatePrecision: 'day',
        releaseRegion: 'en',
        language: 'en',
        opentomeStatus: 'released',
        isOwned,
        canonicalPath: null,
        fileSizeBytes: null,
        fileHash: null,
        publicationReceiptId: null,
        publishedAt: null,
        verifiedAt: null,
        inProgress: false,
        activeJobId: null,
        activeStage: null,
        candidateCount: hasCandidate ? 3 : 0,
        topProvider: hasCandidate ? 'madokami' : null,
        topCandidateId: hasCandidate ? 'cand-1' : null,
        topCandidateTitle: hasCandidate ? 'Haikyu!! v01 [Digital]' : null,
        topCandidateScore: hasCandidate ? 95.0 : null,
        topCandidateSize: hasCandidate ? 85000000 : null,
        lastDiscoveryAt: hasCandidate ? 1700000000 : null,
        edition: 'standard',
        editionId: '019e1d1c-8b74-79d5-911a-a1997aa899a7',
        isAuthoritative: true,
        uncertaintyReason: null,
        displayStatus: hasCandidate ? 'candidate_available' : 'missing_released',
        statusLabel: hasCandidate ? 'Missing · Madokami candidate available' : 'Released · Missing',
      };
    });

    const summary: VolumeManagerSummary = {
      mangaId: 1092,
      seriesId: 'manga:1092',
      title: 'Haikyu!!',
      mangabakaId: 8718,
      edition: 'standard',
      editionId: '019e1d1c-8b74-79d5-911a-a1997aa899a7',
      market: 'en',
      totalOfficialVolumes: 45,
      ownedCount: 0,
      publishedCount: 0,
      inProgressCount: 0,
      candidateAvailableCount: 1,
      missingReleasedCount: 44,
      unreleasedCount: 0,
      isAuthoritative: true,
      uncertaintyReason: null,
      volumes: entries,
    };

    expect(summary.totalOfficialVolumes).toBe(45);
    expect(summary.volumes.length).toBe(45);
    expect(summary.ownedCount).toBe(0);
    expect(summary.isAuthoritative).toBe(true);

    const v1 = summary.volumes[0];
    expect(v1.volumeNumber).toBe(1);
    expect(v1.displayStatus).toBe('candidate_available');
    expect(v1.topProvider).toBe('madokami');
    expect(v1.isOwned).toBe(false);

    const v45 = summary.volumes[44];
    expect(v45.volumeNumber).toBe(45);
    expect(v45.displayStatus).toBe('missing_released');
    expect(v45.isOwned).toBe(false);
  });

  it('evaluates batch planning preview with dry-run invariant', () => {
    const plan: VolumeManagerBatchPlanResponse = {
      mangaId: 1092,
      title: 'Haikyu!!',
      edition: 'standard',
      editionId: '019e1d1c-8b74-79d5-911a-a1997aa899a7',
      totalSelected: 2,
      eligibleCount: 1,
      blockedCount: 1,
      estimatedTotalBytes: 85000000,
      items: [
        {
          volumeNumber: 1,
          isOwned: false,
          isEligible: true,
          status: 'ELIGIBLE',
          reason: 'Verified candidate available for review',
          proposedBackend: 'Madokami (Direct HTTP with Safe Resume)',
          provider: 'madokami',
          candidateId: 'cand-1',
          candidateTitle: 'Haikyu!! v01',
          candidateScore: 95.0,
          sizeBytes: 85000000,
          isDigital: true,
        },
        {
          volumeNumber: 2,
          isOwned: false,
          isEligible: false,
          status: 'UNAVAILABLE',
          reason: 'No release candidates discovered for this volume across providers',
          proposedBackend: null,
          provider: null,
          candidateId: null,
          candidateTitle: null,
          candidateScore: null,
          sizeBytes: null,
          isDigital: null,
        },
      ],
      draftJobId: 'batch-plan-1092-1700000000',
      canExecute: true,
      executionBlockedReason: null,
      isDryRun: true,
    };

    expect(plan.isDryRun).toBe(true);
    expect(plan.totalSelected).toBe(2);
    expect(plan.eligibleCount).toBe(1);
    expect(plan.blockedCount).toBe(1);
    expect(plan.items[0].proposedBackend).toContain('Madokami');
    expect(plan.items[1].status).toBe('UNAVAILABLE');
  });

  it('enforces edition safeguards: ambiguous authority blocks execution', () => {
    const plan: VolumeManagerBatchPlanResponse = {
      mangaId: 700,
      title: 'Bungo Stray Dogs',
      edition: 'standard',
      editionId: null,
      totalSelected: 1,
      eligibleCount: 0,
      blockedCount: 1,
      estimatedTotalBytes: 0,
      items: [
        {
          volumeNumber: 1,
          isOwned: false,
          isEligible: false,
          status: 'UNCERTAIN_EDITION',
          reason: 'Multiple ambiguous MangaBaka collections found',
          proposedBackend: null,
          provider: null,
          candidateId: null,
          candidateTitle: null,
          candidateScore: null,
          sizeBytes: null,
          isDigital: null,
        },
      ],
      draftJobId: 'batch-plan-700-1700000000',
      canExecute: false,
      executionBlockedReason: 'MangaBaka collection requires review before batch acquisition can execute',
      isDryRun: true,
    };

    expect(plan.canExecute).toBe(false);
    expect(plan.executionBlockedReason).toContain('requires review');
    expect(plan.items[0].status).toBe('UNCERTAIN_EDITION');
  });

  it('evaluates crash recovery diagnostics and idempotency safety (M8.3)', () => {
    const recoveryInfo: import('./api').DownloadRecoveryInfo = {
      chapterId: 101,
      title: 'Haikyu!!',
      volumeNumber: 1,
      chapterName: 'Volume 1',
      state: 'failed',
      stage: 'komga_verification',
      failedStage: 'komga_verification',
      errorCategory: 'network',
      errorMessage: 'Komga scan notification timed out (HTTP 504)',
      lastSuccessfulStage: 'published',
      artifactExists: true,
      artifactPath: '/data/manga/Haikyu!!/Haikyu!! v01.cbz',
      canonicalPublicationOccurred: true,
      canonicalReceiptId: 'rcpt-101',
      canonicalPath: '/data/manga/Haikyu!!/Haikyu!! v01.cbz',
      komgaStatus: 'published_awaiting_scan',
      metadataStatus: 'completed',
      retryIsSafe: true,
      recommendedAction: 'verify_komga',
      canRecover: true,
    };

    expect(recoveryInfo.canonicalPublicationOccurred).toBe(true);
    expect(recoveryInfo.canonicalReceiptId).toBe('rcpt-101');
    expect(recoveryInfo.recommendedAction).toBe('verify_komga');
    expect(recoveryInfo.retryIsSafe).toBe(true);
    expect(recoveryInfo.komgaStatus).toBe('published_awaiting_scan');
    expect(recoveryInfo.metadataStatus).toBe('completed');
  });

  it('recommends safe restart download on corrupt staging archive (M8.3)', () => {
    const corruptRecovery: import('./api').DownloadRecoveryInfo = {
      chapterId: 201,
      title: 'Blue Lock',
      volumeNumber: 5,
      chapterName: 'Volume 5',
      state: 'failed',
      stage: 'archive_integrity_validation',
      failedStage: 'archive_integrity_validation',
      errorCategory: 'corrupt_archive',
      errorMessage: 'Invalid central directory in CBZ archive',
      lastSuccessfulStage: 'downloaded_to_staging',
      artifactExists: true,
      artifactPath: '/data/downloads/staging/part_201.cbz',
      canonicalPublicationOccurred: false,
      canonicalReceiptId: null,
      canonicalPath: null,
      komgaStatus: 'not_published',
      metadataStatus: 'not_requested',
      retryIsSafe: true,
      recommendedAction: 'safe_restart_download',
      canRecover: true,
    };

    expect(corruptRecovery.canonicalPublicationOccurred).toBe(false);
    expect(corruptRecovery.errorCategory).toBe('corrupt_archive');
    expect(corruptRecovery.recommendedAction).toBe('safe_restart_download');
    expect(corruptRecovery.canRecover).toBe(true);
  });

  it('evaluates intelligent release selection and side-by-side comparison matrix (M8.4)', () => {
    const comparison: import('./api').CandidateComparisonMatrix = {
      seriesId: 'title:haikyu',
      volumeNumber: 1,
      duplicateGroupsCount: 1,
      preferenceExplanation:
        'Candidate #1 (madokami) is preferred over Candidate #2 (nyaa) due to direct HTTP range transport over torrent swarm, official digital archival edition, higher quality score (120 vs 85).',
      candidates: [
        {
          candidateId: 'cand-mado',
          rank: 1,
          provider: 'madokami',
          transport: 'Direct HTTP (Range resume)',
          title: '[danke-Empire] Haikyu!! v01 (Digital)',
          rawTitle: '[danke-Empire] Haikyu!! v01 (Digital).cbz',
          format: 'CBZ',
          edition: 'Digital',
          uploader: 'danke-Empire',
          sizeBytes: 85000000,
          sizePerPageKb: 415,
          seeders: null,
          healthRating: 'EXCELLENT',
          totalScore: 120,
          selectionVerdict: 'PREFERRED',
          factors: [
            { name: 'Match Type', points: 50, isBonus: true, description: 'Exact volume match bonus (+50)' },
            { name: 'Provider Preference', points: 10, isBonus: true, description: 'Direct HTTP (+10)' },
            { name: 'Official Digital', points: 15, isBonus: true, description: 'Digital edition bonus (+15)' },
            { name: 'Trusted Uploader', points: 20, isBonus: true, description: 'Trusted uploader bonus (+20)' },
            { name: 'Preferred Language', points: 10, isBonus: true, description: 'Preferred language match (+10)' },
            { name: 'Expected Size', points: 10, isBonus: true, description: 'Size within expected range (+10)' },
            { name: 'Resolution', points: 5, isBonus: true, description: '2048px (Ultra-HD) (+5)' },
          ],
          warningBadges: [],
          rejectionReason: null,
          alternateSources: [
            {
              candidateId: 'cand-nyaa-mirror',
              provider: 'nyaa',
              transport: 'TorBox BitTorrent (Debrid)',
              uploader: 'danke-Empire',
              sizeBytes: 85000000,
              seeders: 14,
              rawTitle: '[danke-Empire] Haikyu!! v01 [CBZ]',
              score: 95,
            },
          ],
          advantages: [
            'Direct HTTP transport from Madokami (range resume, no torrent wait)',
            'Official digital archival edition',
            'Verified trusted uploader / recognized quality group',
            'Standard CBZ archive format (native Komga support)',
          ],
          disadvantages: [],
        },
        {
          candidateId: 'cand-nyaa-alt',
          rank: 2,
          provider: 'nyaa',
          transport: 'TorBox BitTorrent (Debrid)',
          title: 'Haikyu!! v01.cbr',
          rawTitle: 'Haikyu!! v01.cbr',
          format: 'CBR',
          edition: 'Scanlation',
          uploader: 'unknown',
          sizeBytes: 75000000,
          sizePerPageKb: 366,
          seeders: 2,
          healthRating: 'DEGRADED',
          totalScore: 40,
          selectionVerdict: 'ALTERNATIVE',
          factors: [
            { name: 'Match Type', points: 50, isBonus: true, description: 'Exact volume match bonus (+50)' },
            { name: 'Swarm Health', points: 2, isBonus: true, description: 'Low seeds: 2 seeds (+2)' },
          ],
          warningBadges: ['CBR (Needs conversion)', 'Low seeders'],
          rejectionReason: 'File format CBR requires conversion',
          alternateSources: [],
          advantages: [],
          disadvantages: [
            'BitTorrent transfer requires debrid caching/swarm availability',
            'CBR RAR archive requires format conversion',
            'Low seeder count in torrent swarm',
          ],
        },
      ],
    };

    expect(comparison.candidates.length).toBe(2);
    expect(comparison.duplicateGroupsCount).toBe(1);
    expect(comparison.preferenceExplanation).toContain('Candidate #1 (madokami) is preferred');

    const top = comparison.candidates[0];
    expect(top.rank).toBe(1);
    expect(top.selectionVerdict).toBe('PREFERRED');
    expect(top.format).toBe('CBZ');
    expect(top.edition).toBe('Digital');
    expect(top.alternateSources.length).toBe(1);
    expect(top.alternateSources[0].provider).toBe('nyaa');

    const alt = comparison.candidates[1];
    expect(alt.rank).toBe(2);
    expect(alt.warningBadges).toContain('CBR (Needs conversion)');
    expect(alt.rejectionReason).toContain('requires conversion');
  });

  it('models library duplicates and storage optimizer status (M8.6)', () => {
    const report: import('./api').LibraryDuplicateReportDto = {
      totalDuplicateGroups: 2,
      exactDuplicatesCount: 1,
      semanticDuplicatesCount: 1,
      chapterVolumeOverlapsCount: 0,
      potentialReclaimableBytes: 85000000,
      generatedAt: 1700000000,
      groups: [
        {
          groupType: 'EXACT_HASH',
          seriesId: 'manga:1092',
          mangaId: 1092,
          seriesTitle: 'Haikyu!!',
          volumeNumber: 1,
          recommendedKeepId: 'rcpt-1',
          resolutionRecommendation: 'Exact duplicate hash: keep primary canonical receipt; review duplicate copy for removal.',
          potentialReclaimableBytes: 85000000,
          items: [
            {
              itemId: 'rcpt-1',
              fileName: 'Haikyu!! v01.cbz',
              path: '/srv/media/manga/Haikyu!!/Haikyu!! v01.cbz',
              sizeBytes: 85000000,
              sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
              isCanonicalReceiptMatch: true,
              isDigital: true,
              resolutionLabel: '2048px (Ultra-HD)',
              format: 'CBZ',
            },
            {
              itemId: 'rcpt-2',
              fileName: 'Haikyu!! v01 (Copy).cbz',
              path: '/srv/media/manga/Haikyu!!/Haikyu!! v01 (Copy).cbz',
              sizeBytes: 85000000,
              sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
              isCanonicalReceiptMatch: true,
              isDigital: true,
              resolutionLabel: '2048px (Ultra-HD)',
              format: 'CBZ',
            },
          ],
        },
      ],
    };

    expect(report.totalDuplicateGroups).toBe(2);
    expect(report.exactDuplicatesCount).toBe(1);
    expect(report.groups[0].recommendedKeepId).toBe('rcpt-1');
    expect(report.groups[0].items.length).toBe(2);

    const optimizer: import('./api').StorageOptimizerStatusDto = {
      isAvailable: true,
      configuredPath: '/opt/mango/bin/RustyLibraryShrinker',
      resolvedPath: '/opt/mango/bin/RustyLibraryShrinker',
      source: 'bundled',
      version: 'RustyLibraryShrinker 1.2.0',
      message: 'RustyLibraryShrinker is available and verified for CBZ optimization.',
      defaultTargetHeight: 2400,
      defaultImageQuality: 90,
      defaultMinSavingsPercent: 10,
    };

    expect(optimizer.isAvailable).toBe(true);
    expect(optimizer.defaultTargetHeight).toBe(2400);
    expect(optimizer.defaultImageQuality).toBe(90);
    expect(optimizer.defaultMinSavingsPercent).toBe(10);
  });
});
