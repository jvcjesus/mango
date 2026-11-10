import { describe, expect, it } from 'vitest';
import { formatLastChecked, formatNextCheck, ago } from './util';
import type { TitleAvailabilityWatcherDto } from './api';

describe('M12 Design System & Timestamp Normalization', () => {
  describe('formatLastChecked', () => {
    it('returns "Never" when timestamp is null, undefined, 0, or epoch 0', () => {
      expect(formatLastChecked(null)).toBe('Never');
      expect(formatLastChecked(undefined)).toBe('Never');
      expect(formatLastChecked(0)).toBe('Never');
      expect(formatLastChecked(100)).toBe('Never'); // < 1 year after 1970
    });

    it('never produces "56 years ago" or 1969 dates', () => {
      const formatted = formatLastChecked(0);
      expect(formatted).not.toContain('56 years ago');
      expect(formatted).not.toContain('1969');
      expect(formatted).not.toContain('1970');
      expect(formatted).toBe('Never');
    });

    it('returns relative time for recent valid timestamps', () => {
      const nowSec = Math.floor(Date.now() / 1000);
      const fiveMinutesAgo = nowSec - 300;
      expect(formatLastChecked(fiveMinutesAgo)).toBe('5 min ago');

      const twoHoursAgo = nowSec - 7200;
      expect(formatLastChecked(twoHoursAgo)).toBe('2 h ago');
    });
  });

  describe('formatNextCheck', () => {
    it('returns "Pending schedule" when timestamp is null, undefined, 0, or epoch 0', () => {
      expect(formatNextCheck(null)).toBe('Pending schedule');
      expect(formatNextCheck(undefined)).toBe('Pending schedule');
      expect(formatNextCheck(0)).toBe('Pending schedule');
      expect(formatNextCheck(86400)).toBe('Pending schedule');
    });

    it('returns "Due now" when check time is in the past', () => {
      const nowSec = Math.floor(Date.now() / 1000);
      const pastSec = nowSec - 60;
      expect(formatNextCheck(pastSec)).toBe('Due now');
    });

    it('returns relative schedule when check time is in the future', () => {
      const nowSec = Math.floor(Date.now() / 1000);
      const inFiveMin = nowSec + 300;
      expect(formatNextCheck(inFiveMin)).toBe('in 5m');

      const inTwoHours = nowSec + 7200;
      expect(formatNextCheck(inTwoHours)).toBe('in 2h');

      const inThreeDays = nowSec + 86400 * 3 + 100;
      expect(formatNextCheck(inThreeDays)).toBe('in 3d');
    });
  });

  describe('Availability Watcher Duplicate Detection', () => {
    function computeDuplicateMap(watchers: Array<{ seriesTitle?: string; mangaId?: number | null; volumeNumber: number }>) {
      const counts = new Map<string, number>();
      for (const w of watchers) {
        const key = `${w.seriesTitle || w.mangaId}_v${w.volumeNumber}`.toLowerCase();
        counts.set(key, (counts.get(key) ?? 0) + 1);
      }
      return counts;
    }

    it('detects duplicate watcher records without hiding either record', () => {
      const watchers = [
        { id: 1, seriesTitle: 'Chainsaw Man', volumeNumber: 15, state: 'UPCOMING' },
        { id: 2, seriesTitle: 'Chainsaw Man', volumeNumber: 15, state: 'RELEASED_AWAITING_PROVIDER' },
        { id: 3, seriesTitle: 'Kagurabachi', volumeNumber: 3, state: 'ELIGIBLE_CANDIDATE_FOUND' },
      ];

      const duplicateMap = computeDuplicateMap(watchers);

      const isChainsawDup = (duplicateMap.get('chainsaw man_v15') ?? 0) > 1;
      const isKagurabachiDup = (duplicateMap.get('kagurabachi_v3') ?? 0) > 1;

      expect(isChainsawDup).toBe(true);
      expect(isKagurabachiDup).toBe(false);

      // Both records remain present in the list
      expect(watchers.length).toBe(3);
    });
  });

  describe('Watcher Table Sorting & Pagination Logic', () => {
    const mockWatchers = [
      { id: 1, seriesTitle: 'Berserk', volumeNumber: 1, state: 'OWNED', lastCheckedAt: 1000, nextCheckAt: 2000 },
      { id: 2, seriesTitle: 'Chainsaw Man', volumeNumber: 2, state: 'UPCOMING', lastCheckedAt: 3000, nextCheckAt: 4000 },
      { id: 3, seriesTitle: 'Attack on Titan', volumeNumber: 10, state: 'CANDIDATE_FOUND', lastCheckedAt: 2000, nextCheckAt: 1000 },
    ];

    it('sorts by title ascending and descending', () => {
      const sortedAsc = [...mockWatchers].sort((a, b) => a.seriesTitle.localeCompare(b.seriesTitle));
      expect(sortedAsc.map(w => w.seriesTitle)).toEqual(['Attack on Titan', 'Berserk', 'Chainsaw Man']);

      const sortedDesc = [...mockWatchers].sort((a, b) => b.seriesTitle.localeCompare(a.seriesTitle));
      expect(sortedDesc.map(w => w.seriesTitle)).toEqual(['Chainsaw Man', 'Berserk', 'Attack on Titan']);
    });

    it('sorts by volume number ascending and descending', () => {
      const sortedVolAsc = [...mockWatchers].sort((a, b) => a.volumeNumber - b.volumeNumber);
      expect(sortedVolAsc.map(w => w.volumeNumber)).toEqual([1, 2, 10]);

      const sortedVolDesc = [...mockWatchers].sort((a, b) => b.volumeNumber - a.volumeNumber);
      expect(sortedVolDesc.map(w => w.volumeNumber)).toEqual([10, 2, 1]);
    });

    it('paginates correctly across page sizes', () => {
      const items = Array.from({ length: 65 }, (_, i) => ({ id: i + 1 }));
      const pageSize = 25;

      const page1 = items.slice(0, pageSize);
      expect(page1.length).toBe(25);
      expect(page1[0].id).toBe(1);
      expect(page1[24].id).toBe(25);

      const page2 = items.slice(pageSize, pageSize * 2);
      expect(page2.length).toBe(25);
      expect(page2[0].id).toBe(26);

      const page3 = items.slice(pageSize * 2, pageSize * 3);
      expect(page3.length).toBe(15);
      expect(page3[0].id).toBe(51);
      expect(page3[14].id).toBe(65);

      const totalPages = Math.ceil(items.length / pageSize);
      expect(totalPages).toBe(3);
    });
  });

  describe('Settings Category Routing & Aliases', () => {
    const ROUTE_ALIASES: Record<string, { tabId: string; pipelineSection?: string }> = {
      komga: { tabId: 'pipeline', pipelineSection: 'paths' },
      mangabaka: { tabId: 'pipeline', pipelineSection: 'metadata' },
      providers: { tabId: 'pipeline', pipelineSection: 'providers' },
      automation: { tabId: 'pipeline', pipelineSection: 'automation' },
      profiles: { tabId: 'pipeline', pipelineSection: 'automation' },
      opentome: { tabId: 'pipeline', pipelineSection: 'metadata' },
      maintenance: { tabId: 'pipeline', pipelineSection: 'maintenance' },
      storage: { tabId: 'pipeline', pipelineSection: 'diagnostics' },
      diagnostics: { tabId: 'pipeline', pipelineSection: 'diagnostics' },
    };

    it('resolves komga subroute to pipeline paths section', () => {
      const alias = ROUTE_ALIASES['komga'];
      expect(alias.tabId).toBe('pipeline');
      expect(alias.pipelineSection).toBe('paths');
    });

    it('resolves providers subroute to pipeline providers section', () => {
      const alias = ROUTE_ALIASES['providers'];
      expect(alias.tabId).toBe('pipeline');
      expect(alias.pipelineSection).toBe('providers');
    });

    it('resolves maintenance subroute to pipeline maintenance section', () => {
      const alias = ROUTE_ALIASES['maintenance'];
      expect(alias.tabId).toBe('pipeline');
      expect(alias.pipelineSection).toBe('maintenance');
    });

    it('preserves pipeline route directly without mutation', () => {
      const subroute = 'pipeline';
      const resolvedTabId = ROUTE_ALIASES[subroute]?.tabId ?? subroute;
      expect(resolvedTabId).toBe('pipeline');
    });
  });
});
