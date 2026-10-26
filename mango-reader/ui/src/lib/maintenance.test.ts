import { afterEach, describe, expect, it, vi } from 'vitest';
import { executeBatchNormalize } from './api';

afterEach(() => vi.unstubAllGlobals());

describe('maintenance mutation boundary', () => {
  it('defaults to dry run and sends only server preview/book identifiers', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ items: [] })));
    vi.stubGlobal('fetch', fetch);
    await executeBatchNormalize('reviewed-preview', [{ komgaBookId: 'book' }]);
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({
      previewId: 'reviewed-preview', bookIds: ['book'], approved: false, dryRun: true,
    });
  });

  it('marks explicitly confirmed mutations as approved', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ items: [] })));
    vi.stubGlobal('fetch', fetch);
    await executeBatchNormalize('reviewed-preview', [{ komgaBookId: 'book' }], false);
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({
      previewId: 'reviewed-preview', bookIds: ['book'], approved: true, dryRun: false,
    });
  });
});
