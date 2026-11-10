import { describe, expect, it } from 'vitest';
import type {
  AutomationOverviewMetrics,
  ProviderHealthSummary,
  VolumeManagerSummary,
} from './api';

describe('M14 Autonomous Acquisition & Self-Healing Pipeline', () => {
  it('models per-series automation overrides in volume manager summary', () => {
    const summary: VolumeManagerSummary = {
      mangaId: 7701,
      seriesId: 'manga:7701',
      title: 'Synthetic Chronicles',
      edition: 'standard',
      market: 'en',
      totalOfficialVolumes: 5,
      ownedCount: 2,
      publishedCount: 2,
      inProgressCount: 0,
      candidateAvailableCount: 2,
      missingReleasedCount: 3,
      unreleasedCount: 0,
      isAuthoritative: true,
      automationOverride: 'auto_acquire',
      effectiveAutomationMode: 'automatic_acquisition',
      volumes: [],
    };

    expect(summary.automationOverride).toBe('auto_acquire');
    expect(summary.effectiveAutomationMode).toBe('automatic_acquisition');
    expect(summary.totalOfficialVolumes).toBe(5);
  });

  it('validates allowed per-series automation override values', () => {
    const validOverrides = [
      'inherit',
      'monitor_only',
      'review_required',
      'auto_acquire',
      'paused',
    ] as const;

    for (const override of validOverrides) {
      const summary: VolumeManagerSummary = {
        mangaId: 101,
        seriesId: 'manga:101',
        title: 'Test Manga',
        edition: 'standard',
        market: 'en',
        totalOfficialVolumes: 1,
        ownedCount: 0,
        publishedCount: 0,
        inProgressCount: 0,
        candidateAvailableCount: 0,
        missingReleasedCount: 1,
        unreleasedCount: 0,
        isAuthoritative: true,
        automationOverride: override,
        effectiveAutomationMode: 'monitor_only',
        volumes: [],
      };
      expect(summary.automationOverride).toBe(override);
    }
  });

  it('models automation overview metrics with provider health summaries', () => {
    const providers: ProviderHealthSummary[] = [
      {
        provider: 'madokami',
        enabled: true,
        configured: true,
        status: 'healthy',
        details: 'Operational',
      },
      {
        provider: 'nyaa',
        enabled: true,
        configured: true,
        status: 'healthy',
        details: 'Operational',
      },
      {
        provider: 'torbox',
        enabled: true,
        configured: true,
        status: 'rate_limited',
        details: 'Rate limited (consecutive failures: 2)',
      },
    ];

    const overview: AutomationOverviewMetrics = {
      automationMode: 'monitor_only',
      autoAcquireEnabled: false,
      operatingMode: 'simulation',
      autoPublishManual: false,
      activeAcquisitionsCount: 0,
      pendingReviewsCount: 12,
      retryScheduledCount: 0,
      blockedAcquisitionsCount: 1,
      awaitingVerificationCount: 0,
      recentlyCompletedCount: 3,
      providerHealth: providers,
    };

    expect(overview.autoAcquireEnabled).toBe(false);
    expect(overview.operatingMode).toBe('simulation');
    expect(overview.providerHealth).toHaveLength(3);
    expect(overview.providerHealth.find((p) => p.provider === 'torbox')?.status).toBe('rate_limited');
  });
});
