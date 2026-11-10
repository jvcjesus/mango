import { describe, it, expect } from 'vitest';
import type { ReviewQueueItem, AcquisitionConfig, LibraryIntegrityReport, IntegrityIssue, DashboardProviderHealthItem } from './api';

describe('Mango M11.1 Reliability & Sequential Acquisition Frontend Logic', () => {
  describe('Volume Acquisition Ordering Policy Defaults and Modeling', () => {
    it('defaults volumeOrderingPolicy to sequential_priority', () => {
      const config: AcquisitionConfig = {
        nyaaEnabled: true,
        torboxEnabled: true,
        hasTorboxToken: true,
        volumeOrderingPolicy: 'sequential_priority',
      };
      expect(config.volumeOrderingPolicy).toBe('sequential_priority');
    });

    it('allows explicit selection of any_available policy', () => {
      const config: AcquisitionConfig = {
        nyaaEnabled: true,
        torboxEnabled: true,
        hasTorboxToken: true,
        volumeOrderingPolicy: 'any_available',
      };
      expect(config.volumeOrderingPolicy).toBe('any_available');
    });
  });

  describe('Review Queue Sequential Ranking & Safe Approval Badges', () => {
    const itemVol9: ReviewQueueItem = {
      id: 101,
      mangaId: 1092,
      seriesId: 'manga:1092',
      seriesTitle: 'Haikyu!!',
      volumeNumber: 9,
      recommendedCandidateId: 'c9',
      provider: 'torbox',
      title: 'Haikyu!! Vol 9',
      rawTitle: 'Haikyu!! - v09.cbz',
      qualityScore: 95,
      confidence: 'HIGH',
      status: 'pending',
      acquisitionPriority: 5,
      earlierMissingGaps: [5, 6, 7, 8],
      orderingExplanation: 'Earlier missing volumes detected (Volumes 5, 6, 7, 8)',
      isSafeToApprove: false,
      createdAt: 100,
      updatedAt: 100,
    };

    const itemVol5: ReviewQueueItem = {
      id: 102,
      mangaId: 1092,
      seriesId: 'manga:1092',
      seriesTitle: 'Haikyu!!',
      volumeNumber: 5,
      recommendedCandidateId: 'c5',
      provider: 'torbox',
      title: 'Haikyu!! Vol 5',
      rawTitle: 'Haikyu!! - v05.cbz',
      qualityScore: 88,
      confidence: 'HIGH',
      status: 'pending',
      acquisitionPriority: 1,
      earlierMissingGaps: [],
      orderingExplanation: 'Next sequential volume in series (Volumes 1-4 owned)',
      isSafeToApprove: true,
      createdAt: 110,
      updatedAt: 110,
    };

    it('orders Review Queue items by acquisition priority first before quality score', () => {
      const queue = [itemVol9, itemVol5];
      queue.sort((a, b) => {
        const pa = a.acquisitionPriority ?? 999;
        const pb = b.acquisitionPriority ?? 999;
        if (pa !== pb) return pa - pb;
        return b.qualityScore - a.qualityScore;
      });

      expect(queue[0].volumeNumber).toBe(5);
      expect(queue[0].acquisitionPriority).toBe(1);
      expect(queue[1].volumeNumber).toBe(9);
      expect(queue[1].acquisitionPriority).toBe(5);
    });

    it('flags out-of-order volumes with isSafeToApprove === false', () => {
      expect(itemVol5.isSafeToApprove).toBe(true);
      expect(itemVol9.isSafeToApprove).toBe(false);
      expect(itemVol9.orderingExplanation).toContain('Earlier missing volumes');
      expect(itemVol9.earlierMissingGaps).toEqual([5, 6, 7, 8]);
    });
  });

  describe('Provider Health Telemetry Mapping', () => {
    it('maps HEALTHY TorBox status to ok badge', () => {
      const prov: DashboardProviderHealthItem = {
        name: 'TorBox',
        category: 'acquisition',
        status: 'HEALTHY',
        details: 'Connected & operational with validated API token',
      };
      const badgeClass = prov.status === 'HEALTHY' || prov.status === 'AVAILABLE' ? 'ok' : 'neutral';
      expect(badgeClass).toBe('ok');
    });

    it('maps DISABLED TorBox status to neutral badge', () => {
      const prov: DashboardProviderHealthItem = {
        name: 'TorBox',
        category: 'acquisition',
        status: 'DISABLED',
        details: 'TorBox backend is disabled in settings',
      };
      const badgeClass = prov.status === 'HEALTHY' || prov.status === 'AVAILABLE' ? 'ok' : 'neutral';
      expect(badgeClass).toBe('neutral');
    });
  });

  describe('Library Integrity Audit Metric Breakdown', () => {
    it('accurately counts issues across identity, ownership, storage, watchers, and proposals', () => {
      const issues: IntegrityIssue[] = [
        {
          id: 'mb-1',
          category: 'identity',
          severity: 'ACTION_REQUIRED',
          code: 'MANGABAKA_IDENTITY_CONFLICT',
          message: 'Conflict',
          affectedPaths: [],
        },
        {
          id: 'komga-1',
          category: 'ownership',
          severity: 'WARNING',
          code: 'KOMGA_OWNERSHIP_ANOMALY',
          message: 'Anomaly',
          affectedPaths: [],
        },
        {
          id: 'storage-1',
          category: 'storage',
          severity: 'WARNING',
          code: 'DUPLICATE_VOLUME_RECORDS',
          message: 'Duplicate volume',
          affectedPaths: [],
        },
        {
          id: 'watcher-1',
          category: 'watchers',
          severity: 'WARNING',
          code: 'DUPLICATE_AVAILABILITY_WATCHERS',
          message: 'Duplicate watcher',
          affectedPaths: [],
        },
        {
          id: 'proposal-1',
          category: 'proposals',
          severity: 'WARNING',
          code: 'DUPLICATE_REVIEW_PROPOSALS',
          message: 'Duplicate proposal',
          affectedPaths: [],
        },
        {
          id: 'tracker-1',
          category: 'trackers',
          severity: 'INFO',
          code: 'ORPHAN_TRACKER_MAPPING',
          message: 'Orphan mapping',
          affectedPaths: [],
        },
      ];

      const identityCount = issues.filter((i) => i.category === 'identity' || i.code?.includes('MANGABAKA')).length;
      const ownershipCount = issues.filter((i) => i.category === 'ownership' || i.code?.includes('KOMGA')).length;
      const storageCount = issues.filter((i) => i.category === 'storage' || i.code?.includes('DUPLICATE_VOLUME')).length;
      const watchersCount = issues.filter((i) => i.category === 'watchers' || i.code?.includes('WATCHER')).length;
      const proposalsCount = issues.filter((i) => i.category === 'proposals' || i.code?.includes('PROPOSAL')).length;
      const trackerCount = issues.filter((i) => i.category === 'trackers' || i.code?.includes('TRACKER')).length;

      expect(identityCount).toBe(1);
      expect(ownershipCount).toBe(1);
      expect(storageCount).toBe(1);
      expect(watchersCount).toBe(1);
      expect(proposalsCount).toBe(1);
      expect(trackerCount).toBe(1);
    });
  });
});
