import { describe, expect, it } from 'vitest';
import type {
  TransferProgressDto,
  DownloadRecoverySummary,
  LibraryIntegrityReport,
} from './api';
import downloadsRaw from '../routes/Downloads.svelte?raw';

describe('Mega Update Milestones v1.6 & v1.7: Diagnostics & Library Health', () => {
  describe('Milestone v1.6: Downloads Lifecycle & Transfer Diagnostics', () => {
    it('models real-time transfer progress and stall diagnostics', () => {
      const diag: TransferProgressDto = {
        chapterId: 2001,
        bytesDownloaded: 25_000_000,
        totalBytes: 50_000_000,
        pagesDone: 100,
        pagesTotal: 200,
        attempt: 2,
        maxAttempts: 5,
        startTime: 1700000000,
        lastProgressTime: 1700000050,
        isStalled: false,
        retryAt: null,
        status: 'downloading',
        error: null,
      };

      expect(diag.chapterId).toBe(2001);
      expect(diag.bytesDownloaded).toBe(25_000_000);
      expect(diag.status).toBe('downloading');
      expect(diag.attempt).toBe(2);
    });

    it('models recoverable failure summary with recovery boundary actions', () => {
      const summary: DownloadRecoverySummary = {
        totalFailedDownloads: 2,
        totalStalledDownloads: 1,
        recoverableDownloadsCount: 2,
        items: [
          {
            chapterId: 1045,
            title: 'Chainsaw Man',
            volumeNumber: 12,
            chapterName: 'Volume 12',
            state: 'failed',
            stage: 'downloading',
            failedStage: 'external_retrieval_stall',
            errorCategory: 'timeout',
            errorMessage: 'TorBox retrieval timed out after 300s',
            lastSuccessfulStage: 'submitted',
            artifactExists: true,
            artifactPath: '/data/staging/csm_12.part',
            canonicalPublicationOccurred: false,
            canonicalReceiptId: null,
            canonicalPath: null,
            komgaStatus: 'not_published',
            metadataStatus: 'not_requested',
            retryIsSafe: true,
            recommendedAction: 'safe_retry_download',
            canRecover: true,
          },
        ],
      };

      expect(summary.recoverableDownloadsCount).toBe(2);
      expect(summary.items[0].canRecover).toBe(true);
      expect(summary.items[0].recommendedAction).toBe('safe_retry_download');
    });

    it('has Transfer Diagnostics and Recoverable Jobs sections in Downloads.svelte', () => {
      expect(downloadsRaw).toContain('TRANSFER LIFECYCLE & WORKER DIAGNOSTICS');
      expect(downloadsRaw).toContain('transferDiagnostics');
      expect(downloadsRaw).toContain('getTransferDiagnostics');
      expect(downloadsRaw).toContain('RECOVERABLE JOB FAILURES');
      expect(downloadsRaw).toContain('getDownloadsRecoverySummary');
    });
  });

  describe('Milestone v1.7: Library Health & Komga Integrity Audits', () => {
    it('models complete library integrity reports with severity breakdown', () => {
      const report: LibraryIntegrityReport = {
        checkedAt: 1700000000,
        status: 'ACTION_REQUIRED',
        totalIssues: 2,
        issuesBySeverity: {
          ACTION_REQUIRED: 1,
          WARNING: 1,
        },
        issues: [
          {
            id: 'integ_1',
            category: 'publication_verification',
            severity: 'ACTION_REQUIRED',
            code: 'unverified_canonical_media',
            message: 'Canonical CBZ on disk has not been verified against Komga',
            details: 'Series ID ser_1066',
            mangaId: 1066,
            seriesTitle: 'Haikyu!!',
            resolutionHint: 'Run Komga library rescan and verify publication receipts',
            affectedPaths: ['/srv/media/manga/Haikyu!!/Haikyu!! v06.cbz'],
          },
        ],
      };

      expect(report.totalIssues).toBe(2);
      expect(report.issues[0].severity).toBe('ACTION_REQUIRED');
      expect(report.issues[0].resolutionHint).toContain('Komga library rescan');
    });

    it('has Library Health & Komga Integrity Audit dashboard in Downloads.svelte', () => {
      expect(downloadsRaw).toContain('LIBRARY HEALTH & KOMGA INTEGRITY AUDIT');
      expect(downloadsRaw).toContain('integrityReport');
      expect(downloadsRaw).toContain('getSystemIntegrity');
    });
  });
});
