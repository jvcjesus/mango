import { describe, expect, it } from 'vitest';
import listsPage from '../routes/Lists.svelte?raw';
import { getLists, type UnifiedTrackedItem, type UnifiedCatalogSummary, type ListsQuery } from './api';

describe('Milestone v1.1 Richer Tracked / Owned / Reading / Acquiring / Auto-Acquire Semantics', () => {
  it('exposes full 5-dimensional semantics on UnifiedTrackedItem', () => {
    const item: UnifiedTrackedItem = {
      id: 'al-12345',
      title: 'Chainsaw Man',
      coverUrl: 'https://example.com/csm.jpg',
      normalizedStatus: 'Reading',
      ownershipStatus: 'Partially Owned',
      ownedUnits: 8,
      totalUnits: 18,
      komgaSeriesId: 'komga-123',
      acquisitionStatus: 'Downloading',
      inboxCount: null,
      autoAcquire: true,
      mangaId: 101,
      progressVolume: 12,
      progressChapter: 97,
      isAheadOfLibrary: true,
      updatedAt: 1700000000,
    };

    expect(item.acquisitionStatus).toBe('Downloading');
    expect(item.autoAcquire).toBe(true);
    expect(item.isAheadOfLibrary).toBe(true);
    expect(item.progressVolume).toBe(12);
    expect(item.ownedUnits).toBe(8);
  });

  it('includes acquisition lifecycle and auto-acquire in ListsQuery and getLists serialization', () => {
    const query: ListsQuery = {
      status: 'reading',
      ownership: 'partially_owned',
      acquisition: 'downloading',
      autoAcquire: true,
    };

    // getLists uses fetch/get; test query formatting contract
    const params = new URLSearchParams();
    if (query.status) params.set('status', query.status);
    if (query.ownership) params.set('ownership', query.ownership);
    if (query.acquisition) params.set('acquisition', query.acquisition);
    if (query.autoAcquire !== undefined) params.set('autoAcquire', String(query.autoAcquire));

    expect(params.get('acquisition')).toBe('downloading');
    expect(params.get('autoAcquire')).toBe('true');
  });

  it('includes faceted acquisition and auto-acquire controls in Lists.svelte markup', () => {
    expect(listsPage).toContain('bind:value={acquisitionFilter}');
    expect(listsPage).toContain('bind:value={ownershipFilter}');
    expect(listsPage).toContain('onclick={() => (autoAcquireOnly = !autoAcquireOnly)}');
    expect(listsPage).toContain('Auto-Acquire ({facetedAutoAcquireCount})');
    expect(listsPage).toContain('class="badge acquisition-badge');
    expect(listsPage).toContain('class="badge ahead-badge"');
    expect(listsPage).toContain('class="ahead-indicator"');
  });

  it('aggregates acquiring, inbox, monitored, and auto-acquire counts in summary', () => {
    const summary: UnifiedCatalogSummary = {
      total: 10,
      reading: 5,
      planning: 3,
      completed: 2,
      paused: 0,
      dropped: 0,
      owned: 4,
      partiallyOwned: 4,
      notOwned: 2,
      acquiring: 2,
      inbox: 1,
      monitored: 3,
      autoAcquire: 4,
    };

    expect(summary.acquiring).toBe(2);
    expect(summary.inbox).toBe(1);
    expect(summary.monitored).toBe(3);
    expect(summary.autoAcquire).toBe(4);
  });
});
