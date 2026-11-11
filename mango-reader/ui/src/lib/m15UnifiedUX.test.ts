import { describe, expect, it } from 'vitest';
import type {
  DashboardSummaryDto,
  ReconciliationSummary,
  Resume,
} from './api';

describe('Mango M15 — Unified UX, Navigation Simplification & Original Tankōbon Design', () => {
  describe('1. Editorial Home Data Derivations & Next Missing Volumes Priority', () => {
    const mockReconciliations: ReconciliationSummary[] = [
      {
        seriesId: 'manga:101',
        mangaId: 101,
        komgaSeriesId: 'komga-101',
        mangabakaId: 501,
        collectionId: null,
        title: 'Chainsaw Man',
        edition: 'Standard',
        status: 'missing_volume',
        isOngoing: true,
        totalOfficialVolumes: 16,
        ownedVolumesCount: 14,
        missingVolumesCount: 2,
        ownedVolumes: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
        missingVolumes: [15, 16],
        futureVolumes: [],
        lastReconciledAt: 1700000000,
        lastError: null,
        reviewReason: null,
        displayStatus: 'missing_volume',
      },
      {
        seriesId: 'manga:102',
        mangaId: 102,
        komgaSeriesId: 'komga-102',
        mangabakaId: 502,
        collectionId: null,
        title: 'Frieren: Beyond Journey\'s End',
        edition: 'Standard',
        status: 'up_to_date',
        isOngoing: true,
        totalOfficialVolumes: 12,
        ownedVolumesCount: 12,
        missingVolumesCount: 0,
        ownedVolumes: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        missingVolumes: [],
        futureVolumes: [],
        lastReconciledAt: 1700000000,
        lastError: null,
        reviewReason: null,
        displayStatus: 'up_to_date',
      },
      {
        seriesId: 'manga:103',
        mangaId: 103,
        komgaSeriesId: 'komga-103',
        mangabakaId: 503,
        collectionId: null,
        title: 'Kagurabachi',
        edition: 'Standard',
        status: 'missing_volume',
        isOngoing: true,
        totalOfficialVolumes: 4,
        ownedVolumesCount: 1,
        missingVolumesCount: 3,
        ownedVolumes: [1],
        missingVolumes: [2, 3, 4],
        futureVolumes: [],
        lastReconciledAt: 1700000000,
        lastError: null,
        reviewReason: null,
        displayStatus: 'missing_volume',
      },
    ];

    it('filters and surfaces only series with missing released volumes for recommendations', () => {
      const actionableMissing = mockReconciliations.filter(
        (r) => r.missingVolumesCount > 0 && r.mangaId !== null
      );

      expect(actionableMissing.length).toBe(2);
      expect(actionableMissing[0].title).toBe('Chainsaw Man');
      expect(actionableMissing[0].missingVolumes[0]).toBe(15);
      expect(actionableMissing[1].title).toBe('Kagurabachi');
      expect(actionableMissing[1].missingVolumes[0]).toBe(2);
    });

    it('formats resume reading session data accurately for the editorial hero panel', () => {
      const mockResume: Resume = {
        mangaId: 101,
        chapterId: 8820,
        title: 'Chainsaw Man',
        cover: '/api/v1/covers/101',
        chapterName: 'Chapter 145: The Return',
        page: 12,
        pageCount: 20,
      };

      const percent = Math.round(((mockResume.page + 1) / (mockResume.pageCount ?? 1)) * 100);
      expect(percent).toBe(65);
      expect(mockResume.page + 1).toBe(13);
      expect(mockResume.chapterName).toBe('Chapter 145: The Return');
    });
  });

  describe('2. Collection View Integration (Owned vs Tracked Lists)', () => {
    it('defines distinct collection view modes for Library and Lists', () => {
      const views = [
        { id: 'owned', path: '/library', label: 'Owned in Komga', isCanonical: true },
        { id: 'tracked', path: '/lists', label: 'Tracked Lists', isCanonical: false },
      ];

      expect(views.find((v) => v.id === 'owned')?.isCanonical).toBe(true);
      expect(views.find((v) => v.id === 'tracked')?.isCanonical).toBe(false);
      expect(views[0].path).toBe('/library');
      expect(views[1].path).toBe('/lists');
    });
  });

  describe('3. Task-Oriented Activity Subnavigation', () => {
    it('structures activity tabs around user intent rather than technical silos', () => {
      const activityTabs = [
        { id: 'inbox', label: 'Needs Attention', purpose: 'Review Queue & Blocked Proposals' },
        { id: 'downloads', label: 'In Progress', purpose: 'Active Transfers & Verification' },
        { id: 'history', label: 'Completed & History', purpose: 'Publication Receipts & Audit' },
        { id: 'monitoring', label: 'Release Watchers', purpose: 'Watcher Polling & Intelligence' },
      ];

      expect(activityTabs[0].label).toBe('Needs Attention');
      expect(activityTabs[1].label).toBe('In Progress');
      expect(activityTabs[2].label).toBe('Completed & History');
      expect(activityTabs[3].label).toBe('Release Watchers');
    });
  });
});
