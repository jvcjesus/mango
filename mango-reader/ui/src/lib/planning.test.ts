import { describe, it, expect, vi } from 'vitest';
import { PlanningSession, parseRequestedVolumes } from './planning';

describe('interactive planner lifecycle', () => {
  it.each([['plan', { selectedItems: [1] }], ['no results', { selectedItems: [] }]])('settles %s', async (_, value) => {
    const result = await new PlanningSession().run(async () => value);
    expect(result).toEqual({status:'ready', value});
  });
  it('settles provider errors', async () => {
    const result = await new PlanningSession().run(async () => { throw new Error('Nyaa unavailable'); });
    expect(result.status).toBe('failed');
  });
  it('settles even if the transport never resolves', async () => {
    vi.useFakeTimers();
    const pending = new PlanningSession().run(() => new Promise(() => {}), 50);
    await vi.advanceTimersByTimeAsync(51);
    expect((await pending).status).toBe('failed');
    vi.useRealTimers();
  });
  it('cancel invalidates pending work and aborts its signal', async () => {
    const session = new PlanningSession(); let signal: AbortSignal | undefined;
    const pending = session.run(s => { signal = s; return new Promise(() => {}); });
    session.cancel();
    expect(signal?.aborted).toBe(true);
    expect((await pending).status).toBe('stale');
  });
  it('old responses cannot replace a newer result', async () => {
    const session = new PlanningSession(); let settle!: (value:string) => void;
    const old = session.run<string>(() => new Promise(resolve => { settle = resolve; }));
    const current = await session.run(async () => 'new title');
    settle('old title');
    expect((await old).status).toBe('stale');
    expect(current).toEqual({status:'ready', value:'new title'});
  });
  it('retry after error starts a fresh request', async () => {
    const session = new PlanningSession();
    expect((await session.run(async () => { throw new Error('offline'); })).status).toBe('failed');
    expect(await session.run(async () => 'fresh plan')).toEqual({status:'ready', value:'fresh plan'});
  });
  it('bounds malformed, infinite and huge ranges', () => {
    expect(parseRequestedVolumes('1, 4-6, 39')).toEqual([1,4,5,6,39]);
    for (const invalid of ['', 'Infinity', '1-100000000000', '4-1', 'NaN']) expect(() => parseRequestedVolumes(invalid)).toThrow();
  });
});

it('planning API carries edition intent and cancellation', async () => {
  const { planByTitle } = await import('./api');
  const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({selectedItems:[]}), {status:200}));
  vi.stubGlobal('fetch', fetchMock);
  try {
    const controller = new AbortController();
    await planByTitle({title:'Series X', requestedEdition:'master', selectedReleaseId:'release4', useReleaseAnyway:true}, controller.signal);
    const options = fetchMock.mock.calls[0][1];
    expect(options.signal).toBe(controller.signal);
    expect(JSON.parse(options.body)).toMatchObject({requestedEdition:'master', selectedReleaseId:'release4', useReleaseAnyway:true});
  } finally { vi.unstubAllGlobals(); }
});
