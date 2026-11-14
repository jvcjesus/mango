import { describe, expect, it } from 'vitest';
import { resolveVolumePresentation } from './volumeStatus';
import type { MissingContentSummary, SeriesGapAnalysisDto, UnitStatusEntry } from './api';
import titleDiagnosticsModalRaw from './TitleDiagnosticsModal.svelte?raw';
import seriesSummaryCardRaw from './SeriesSummaryCard.svelte?raw';
import titlePageRaw from '../routes/Title.svelte?raw';

describe('Milestone v1.2 Missing-Content Intelligence, Gap Classification & Multi-Edition Resolution', () => {
  it('exposes gap classification and future volume fields on MissingContentSummary', () => {
    const summary: MissingContentSummary = {
      mangaId: 1066,
      title: 'Haikyu!!',
      totalKnownVolumes: 45,
      ownedVolumes: [1, 2, 5, 6, 7, 8],
      missingVolumes: [3, 4, 10, 11],
      acquiringVolumes: [],
      unavailableVolumes: [],
      unknownVolumes: [],
      futureVolumes: [12, 13],
      interiorGaps: [3, 4],
      leadingGaps: [],
      trailingGaps: [10, 11],
      hasInteriorGaps: true,
      edition: 'Standard Tankobon',
      collectionId: 'col-mb-8718',
      volumeEntries: [],
      totalKnownChapters: 0,
      ownedChapters: [],
      missingChapters: [],
      acquiringChapters: [],
      isFullyOwned: false,
      hasMissing: true,
      explanation: 'Missing 4 volumes (2 interior gaps: v3–4)',
    };

    expect(summary.hasInteriorGaps).toBe(true);
    expect(summary.interiorGaps).toEqual([3, 4]);
    expect(summary.trailingGaps).toEqual([10, 11]);
    expect(summary.futureVolumes).toEqual([12, 13]);
    expect(summary.edition).toBe('Standard Tankobon');
    expect(summary.collectionId).toBe('col-mb-8718');
  });

  it('supports unreleased status on UnitStatusEntry', () => {
    const entry: UnitStatusEntry = {
      unit: 12,
      state: 'unreleased',
      evidence: 'OpenTome schedule',
    };

    expect(entry.state).toBe('unreleased');
    expect(entry.unit).toBe(12);
  });

  it('includes unreleasedCount in SeriesGapAnalysisDto', () => {
    const analysis: SeriesGapAnalysisDto = {
      totalVolumes: 12,
      ownedCount: 5,
      missingCount: 5,
      unreleasedCount: 2,
      interiorGapCount: 2,
      leadingGapCount: 0,
      trailingGapCount: 3,
      hasInteriorGaps: true,
      gapVolumeNumbers: [3, 4, 10, 11, 12],
      summaryText: 'Interior gap detected: Vol. 3–4 missing between owned volumes (2 upcoming)',
    };

    expect(analysis.unreleasedCount).toBe(2);
    expect(analysis.hasInteriorGaps).toBe(true);
  });

  it('resolves presentation for upcoming unreleased volume correctly', () => {
    const pres = resolveVolumePresentation({
      volume: 12,
      isOwnedInKomga: false,
      isUpcoming: true,
      releaseDate: '2026-12-01',
    });

    expect(pres.status).toBe('upcoming');
    expect(pres.statusLabel).toBe('Upcoming (2026-12-01)');
    expect(pres.badgeVariant).toBe('neutral');
    expect(pres.physicalState).toBe('unreleased');
    expect(pres.canAcquire).toBe(false);
    expect(pres.isUpcoming).toBe(true);
    expect(pres.releaseDate).toBe('2026-12-01');
    expect(pres.physicalLabel).toContain('Scheduled release: 2026-12-01');
  });

  it('resolves presentation for interior gap with high priority alert', () => {
    const pres = resolveVolumePresentation({
      volume: 3,
      isOwnedInKomga: false,
      isMissingReported: true,
      isInteriorGap: true,
    });

    expect(pres.status).toBe('missing');
    expect(pres.statusLabel).toBe('Interior Gap (Missing)');
    expect(pres.isInteriorGap).toBe(true);
    expect(pres.canAcquire).toBe(true);
  });

  it('surfaces unreleased future volume indicator in TitleDiagnosticsModal', () => {
    expect(titleDiagnosticsModalRaw).toContain('UNRELEASED_FUTURE');
    expect(titleDiagnosticsModalRaw).toContain('unreleased-pill');
    expect(titleDiagnosticsModalRaw).toContain('UPCOMING');
  });

  it('surfaces interior reading gap alert and upcoming count in SeriesSummaryCard', () => {
    expect(seriesSummaryCardRaw).toContain('interior-gap-alert-row');
    expect(seriesSummaryCardRaw).toContain('gap-alert-badge');
    expect(seriesSummaryCardRaw).toContain('upcoming-accent');
    expect(seriesSummaryCardRaw).toContain('hasInteriorGaps');
  });

  it('wires up missing intelligence into Title.svelte volume presentation', () => {
    expect(titlePageRaw).toContain('isUpcoming');
    expect(titlePageRaw).toContain('isInteriorGap');
    expect(titlePageRaw).toContain('missingContent={missingContent}');
  });
});
