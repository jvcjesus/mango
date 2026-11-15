import { describe, expect, it } from 'vitest';
import type {
  AcquisitionProfile,
  SourceHealthReport,
  MetadataReviewItem,
  ResolveReviewRequest,
} from './api';
import settingsRaw from '../routes/Settings.svelte?raw';

describe('Mega Update Core Bundle: Milestones v1.3, v1.4, and v1.5', () => {
  describe('Milestone v1.3: Acquisition Profiles', () => {
    it('supports rich acquisition profile models with quality preferences and source rules', () => {
      const profile: AcquisitionProfile = {
        id: 'prof_archival',
        name: 'Archival Master Quality',
        isDefault: false,
        language: 'en',
        preferredReleaseType: 'volume',
        sourceSelection: 'smart',
        preferredSources: ['nyaa', 'madokami'],
        preferredUploaders: ['danke-Empire', 'LuGit'],
        minSeeders: 3,
        preferDigital: true,
        trustedOnly: true,
        collectionPolicy: 'prefer_individual',
        maxUnnecessaryDuplicateCoverage: 1,
        maxAcquisitionSizeBytes: 2_000_000_000,
        maxPlanSizeBytes: 10_000_000_000,
        qualityPreference: 'best',
        allowMixedCollections: false,
        allowZeroSeeders: 'no',
        allowUnsupportedEditions: false,
        autoAcquire: false,
        createdAt: 1700000000,
        updatedAt: 1700000000,
      };

      expect(profile.qualityPreference).toBe('best');
      expect(profile.preferredUploaders).toContain('danke-Empire');
      expect(profile.collectionPolicy).toBe('prefer_individual');
      expect(profile.minSeeders).toBe(3);
      expect(profile.autoAcquire).toBe(false);
    });

    it('has acquisition profile management controls in Settings.svelte', () => {
      expect(settingsRaw).toContain('Acquisition Profiles');
      expect(settingsRaw).toContain('handleDuplicateProfile');
      expect(settingsRaw).toContain('saveEditingProfile');
      expect(settingsRaw).toContain('handleDeleteProfile');
      expect(settingsRaw).toContain('handleSetDefaultProfile');
    });
  });

  describe('Milestone v1.4: Source Intelligence & Uploader Reliability', () => {
    it('models source health and historical uploader reliability telemetry', () => {
      const report: SourceHealthReport = {
        sourceId: 'nyaa',
        name: 'Nyaa Releases',
        supportsVolumes: true,
        supportsChapters: false,
        supportsCollections: true,
        requiresExternalBackend: true,
        status: 'healthy',
        successCount: 42,
        failureCount: 1,
        lastResponseTimeMs: 145,
        lastError: null,
        lastSuccessAt: 1700000000,
        exactMatchCount: 38,
        totalCandidatesFound: 120,
        uploaderStats: {
          'danke-Empire': {
            validAcquisitions: 19,
            attemptedAcquisitions: 20,
            reliabilityPercent: 95.0,
          },
          'UnknownScanner': {
            validAcquisitions: 1,
            attemptedAcquisitions: 3,
            reliabilityPercent: 33.3,
          },
        },
        updatedAt: 1700000000,
      };

      expect(report.status).toBe('healthy');
      expect(report.lastResponseTimeMs).toBe(145);
      expect(report.uploaderStats['danke-Empire'].reliabilityPercent).toBe(95.0);
      expect(report.uploaderStats['UnknownScanner'].reliabilityPercent).toBeLessThan(50);
    });

    it('has Source Intelligence & Health dashboard section in Settings.svelte', () => {
      expect(settingsRaw).toContain('Acquisition Sources & Health');
      expect(settingsRaw).toContain('sourceHealthReports');
      expect(settingsRaw).toContain('loadSourceHealth');
    });
  });

  describe('Milestone v1.5: Metadata Review & Disambiguation Queue', () => {
    it('models metadata review queue items and disambiguation actions', () => {
      const item: MetadataReviewItem = {
        id: 'rev_1066',
        mangaId: 1066,
        title: 'Haikyu!!',
        coverUrl: '/covers/1066.jpg',
        issueType: 'ambiguous_identity',
        status: 'pending',
        reason: 'Ambiguous MangaBaka identity: 2 candidates found',
        mangabakaId: null,
        currentEdition: 'standard',
        suggestedMangabakaId: 8718,
        suggestedEdition: 'standard',
        details: { reconciliationStatus: 'review_required' },
        createdAt: 1700000000,
      };

      const req: ResolveReviewRequest = {
        action: 'link_mangabaka',
        mangabakaId: 8718,
      };

      expect(item.issueType).toBe('ambiguous_identity');
      expect(item.status).toBe('pending');
      expect(req.action).toBe('link_mangabaka');
      expect(req.mangabakaId).toBe(8718);
    });

    it('has Metadata Review Queue UI card in Settings.svelte', () => {
      expect(settingsRaw).toContain('Metadata Review & Disambiguation Queue');
      expect(settingsRaw).toContain('metadataReviewQueue');
      expect(settingsRaw).toContain('loadMetadataReviewQueue');
      expect(settingsRaw).toContain('handleResolveReview');
    });
  });
});
