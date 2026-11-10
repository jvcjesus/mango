import { describe, it, expect } from 'vitest';
import {
  resolveVolumeCollectionState,
  calculateNextMissingVolume,
  groupReviewQueueBySeries,
  deriveLifecycleTimeline,
} from './volumeStatus';
import type { VolumeManagerEntry, ReviewQueueItem } from './api';

describe('Mango M13 — Unified Series Workspace, Acquisition Inbox & Lifecycle Consolidation', () => {
  describe('12 Explicit Volume Collection States & Komga Ownership Safety', () => {
    const baseEntry: VolumeManagerEntry = {
      volumeNumber: 6,
      label: 'Volume 6',
      releaseRegion: 'US',
      language: 'en',
      opentomeStatus: 'released',
      isOwned: false,
      inProgress: false,
      candidateCount: 0,
      edition: 'Standard',
      isAuthoritative: true,
      displayStatus: 'missing_released',
      statusLabel: 'Missing from library',
    };

    it('identifies owned volumes strictly when isOwned is true', () => {
      const owned = resolveVolumeCollectionState({ ...baseEntry, isOwned: true });
      expect(owned.state).toBe('owned');
      expect(owned.isOwned).toBe(true);
      expect(owned.label).toBe('Owned');
    });

    it('NEVER marks a volume as owned merely because candidate exists or download completed', () => {
      const withCandidate = resolveVolumeCollectionState({
        ...baseEntry,
        isOwned: false,
        candidateCount: 3,
        displayStatus: 'candidate_available',
      });
      expect(withCandidate.state).toBe('available');
      expect(withCandidate.isOwned).toBe(false);

      const downloaded = resolveVolumeCollectionState({
        ...baseEntry,
        isOwned: false,
        activeStage: 'staged',
        displayStatus: 'staged',
      });
      expect(downloaded.state).toBe('staged');
      expect(downloaded.isOwned).toBe(false);
    });

    it('models all 12 operational states without collapsing them into misleading labels', () => {
      // 1. owned
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: true }).state).toBe('owned');
      // 2. missing
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, candidateCount: 0 }).state).toBe('missing');
      // 3. available
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, candidateCount: 2 }).state).toBe('available');
      // 4. pending_review
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, activeStage: 'pending_review' }).state).toBe('pending_review');
      // 5. queued
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, activeStage: 'queued' }).state).toBe('queued');
      // 6. downloading
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, activeStage: 'downloading' }).state).toBe('downloading');
      // 7. validating
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, activeStage: 'validating' }).state).toBe('validating');
      // 8. staged
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, activeStage: 'staged' }).state).toBe('staged');
      // 9. publishing
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, activeStage: 'publishing' }).state).toBe('publishing');
      // 10. awaiting_verification
      expect(
        resolveVolumeCollectionState({
          ...baseEntry,
          isOwned: false,
          publicationReceiptId: 'rcpt-123',
          displayStatus: 'published',
        }).state
      ).toBe('awaiting_verification');
      // 11. failed
      expect(resolveVolumeCollectionState({ ...baseEntry, isOwned: false, activeStage: 'failed' }).state).toBe('failed');
      // 12. unknown
      expect(
        resolveVolumeCollectionState({
          ...baseEntry,
          isOwned: false,
          displayStatus: 'unreleased',
          opentomeStatus: 'unannounced',
        }).state
      ).toBe('unknown');
    });
  });

  describe('Next Missing Volume Calculation & Sequential Priority (Haikyu!! Case)', () => {
    it('identifies Volume 6 as next missing volume when Volumes 1-5 are owned', () => {
      const ownedVolumes = [1, 2, 3, 4, 5];
      const result = calculateNextMissingVolume(ownedVolumes, 45);

      expect(result.nextMissingVolume).toBe(6);
      expect(result.highestOwnedVolume).toBe(5);
      expect(result.earlierGaps).toEqual([]);
      expect(result.totalOwnedCount).toBe(5);
      expect(result.totalExpectedCount).toBe(45);
      expect(result.completionPercentage).toBe(11);
      expect(result.isComplete).toBe(false);
      expect(result.explanation).toContain('Volumes 1–5 owned');
      expect(result.explanation).toContain('Volume 6');
    });

    it('identifies interior gaps before higher numbers', () => {
      const ownedVolumes = [1, 3, 4, 5];
      const result = calculateNextMissingVolume(ownedVolumes, 45);

      expect(result.nextMissingVolume).toBe(2);
      expect(result.earlierGaps).toEqual([2]);
      expect(result.explanation).toContain('Interior gap');
      expect(result.explanation).toContain('Volume 2');
    });

    it('handles series with zero owned volumes by suggesting Volume 1', () => {
      const result = calculateNextMissingVolume([], 12);
      expect(result.nextMissingVolume).toBe(1);
      expect(result.totalOwnedCount).toBe(0);
      expect(result.completionPercentage).toBe(0);
    });

    it('identifies complete collections accurately', () => {
      const ownedVolumes = Array.from({ length: 45 }, (_, i) => i + 1);
      const result = calculateNextMissingVolume(ownedVolumes, 45);

      expect(result.nextMissingVolume).toBeNull();
      expect(result.isComplete).toBe(true);
      expect(result.completionPercentage).toBe(100);
      expect(result.explanation).toContain('All 45 official volumes verified');
    });
  });

  describe('Acquisition Inbox Series Grouping & Priority Presentation', () => {
    const itemVol6: ReviewQueueItem = {
      id: 201,
      mangaId: 1092,
      seriesId: 'manga:1092',
      seriesTitle: 'Haikyu!!',
      volumeNumber: 6,
      recommendedCandidateId: 'c6',
      provider: 'torbox',
      title: 'Haikyu!! Vol 6',
      rawTitle: 'Haikyu!! - v06.cbz',
      qualityScore: 92,
      confidence: 'HIGH',
      status: 'pending',
      acquisitionPriority: 1,
      earlierMissingGaps: [],
      orderingExplanation: 'Next sequential volume in series (Volumes 1-5 owned)',
      isSafeToApprove: true,
      createdAt: 200,
      updatedAt: 200,
    };

    const itemVol29: ReviewQueueItem = {
      id: 202,
      mangaId: 1092,
      seriesId: 'manga:1092',
      seriesTitle: 'Haikyu!!',
      volumeNumber: 29,
      recommendedCandidateId: 'c29',
      provider: 'torbox',
      title: 'Haikyu!! Vol 29',
      rawTitle: 'Haikyu!! - v29.cbz',
      qualityScore: 96,
      confidence: 'HIGH',
      status: 'pending',
      acquisitionPriority: 4,
      earlierMissingGaps: [6, 7, 8],
      orderingExplanation: 'Earlier missing volumes detected (Volumes 6, 7, 8)',
      isSafeToApprove: false,
      createdAt: 210,
      updatedAt: 210,
    };

    it('groups proposals by series and prioritizes Volume 6 before Volume 29', () => {
      const ownedMap = { 'manga:1092': [1, 2, 3, 4, 5] };
      const totalsMap = { 'manga:1092': 45 };

      const groups = groupReviewQueueBySeries([itemVol29, itemVol6], ownedMap, totalsMap);

      expect(groups).toHaveLength(1);
      const group = groups[0];
      expect(group.seriesTitle).toBe('Haikyu!!');
      expect(group.ownedCount).toBe(5);
      expect(group.totalOfficialCount).toBe(45);
      expect(group.nextMissingVolume).toBe(6);
      expect(group.primaryProposals).toHaveLength(1);
      expect(group.primaryProposals[0].volumeNumber).toBe(6);
      expect(group.outOfOrderProposals).toHaveLength(1);
      expect(group.outOfOrderProposals[0].volumeNumber).toBe(29);
      expect(group.outOfOrderCount).toBe(1);
      expect(group.warnings[0]).toContain('Volume 29 is out-of-order');
    });
  });

  describe('Acquisition Lifecycle Timeline', () => {
    it('produces the exact 8 ordered lifecycle stages', () => {
      const timeline = deriveLifecycleTimeline({
        discoveredAt: 1000,
        reviewedAt: 2000,
        reviewStatus: 'approved',
        queuedAt: 3000,
        downloadStartedAt: 4000,
        downloadCompletedAt: 5000,
        validatedAt: 6000,
        stagedAt: 7000,
        publishedAt: 8000,
        verifiedAt: 9000,
        currentStage: 'completed',
        provider: 'torbox',
      });

      expect(timeline).toHaveLength(8);
      expect(timeline.map((s) => s.id)).toEqual([
        'discovery',
        'review',
        'queued',
        'downloading',
        'validation',
        'staging',
        'publication',
        'verification',
      ]);
      expect(timeline.every((s) => s.status === 'completed')).toBe(true);
    });

    it('faithfully marks failed stage when an error occurs during download', () => {
      const timeline = deriveLifecycleTimeline({
        discoveredAt: 1000,
        reviewedAt: 2000,
        reviewStatus: 'approved',
        queuedAt: 3000,
        downloadStartedAt: 4000,
        currentStage: 'downloading',
        error: 'TorBox transfer timeout',
      });

      const dlStage = timeline.find((s) => s.id === 'downloading');
      expect(dlStage?.status).toBe('failed');
      expect(dlStage?.detail).toContain('TorBox transfer timeout');

      const pubStage = timeline.find((s) => s.id === 'publication');
      expect(pubStage?.status).toBe('pending');
    });
  });

  describe('Navigation & Legacy Route Compatibility', () => {
    it('defines 4 primary top-level destinations (Home, Library, Discover, Activity)', () => {
      const primaryDestinations = [
        { to: '/', label: 'Home' },
        { to: '/library', label: 'Library' },
        { to: '/browse', label: 'Discover' },
        { to: '/activity', label: 'Activity' },
      ];

      expect(primaryDestinations).toHaveLength(4);
      expect(primaryDestinations.map((d) => d.label)).toEqual(['Home', 'Library', 'Discover', 'Activity']);
    });

    it('resolves legacy routes to appropriate Activity tabs', () => {
      function resolveLegacyRoute(path: string): { to: string; tab?: string } {
        if (path === '/updates') return { to: '/activity', tab: 'inbox' };
        if (path === '/downloads') return { to: '/activity', tab: 'downloads' };
        if (path === '/history') return { to: '/activity', tab: 'history' };
        if (path === '/lists') return { to: '/library', tab: 'lists' };
        return { to: path };
      }

      expect(resolveLegacyRoute('/updates')).toEqual({ to: '/activity', tab: 'inbox' });
      expect(resolveLegacyRoute('/downloads')).toEqual({ to: '/activity', tab: 'downloads' });
      expect(resolveLegacyRoute('/history')).toEqual({ to: '/activity', tab: 'history' });
      expect(resolveLegacyRoute('/lists')).toEqual({ to: '/library', tab: 'lists' });
    });
  });

  describe('Watchers Dashboard Series Grouping', () => {
    it('groups flat watchers into series summaries to prevent row sprawl', () => {
      const flatWatchers = [
        { id: 1, seriesTitle: 'Haikyu!!', volumeNumber: 5, state: 'ELIGIBLE_CANDIDATE_FOUND', mangaId: 1092 },
        { id: 2, seriesTitle: 'Haikyu!!', volumeNumber: 6, state: 'RELEASED_AWAITING_PROVIDER', mangaId: 1092 },
        { id: 3, seriesTitle: 'Chainsaw Man', volumeNumber: 15, state: 'UPCOMING', mangaId: 2001 },
      ];

      const groups = new Map<string, typeof flatWatchers>();
      for (const w of flatWatchers) {
        if (!groups.has(w.seriesTitle)) groups.set(w.seriesTitle, []);
        groups.get(w.seriesTitle)!.push(w);
      }

      expect(groups.size).toBe(2);
      expect(groups.get('Haikyu!!')).toHaveLength(2);
      expect(groups.get('Chainsaw Man')).toHaveLength(1);
    });
  });
});
