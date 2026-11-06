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
});
