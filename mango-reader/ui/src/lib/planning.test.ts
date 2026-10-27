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

it('source-local volume choices come only from eligible filename evidence', async () => {
  const { discoveredVolumes } = await import('./planning');
  const units: import('./api').SourceLocalUnit[] = [6,1,4,4,2,3,5].map(n => ({unit:{type:'volume',value:n},edition:'master',archive:{path:`Master v${n}.cbz`,size:100},eligible:true,evidence:'Filename'}));
  units.push({unit:{type:'unknown'},edition:'master',archive:{path:'file12.cbz',size:100},eligible:false,evidence:'Unknown'});
  units.push({unit:{type:'chapter',value:39},edition:'master',archive:{path:'ch39.cbz',size:100},eligible:true,evidence:'Chapter'});
  expect(discoveredVolumes(units)).toEqual([1,2,3,4,5,6]);
  expect(discoveredVolumes([])).toEqual([]);
});

it('specific source archive and override remain explicit in regenerated requests', async () => {
  const { planByTitle } = await import('./api');
  const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({selectedItems:[]}), {status:200}));
  vi.stubGlobal('fetch', fetchMock);
  try {
    await planByTitle({title:'Series',contentMode:'specific',requestedVolumes:[4],requestedEdition:'master',selectedReleaseId:'release',selectedArchive:{path:'Master/Alternative v04.cbz',size:400},useReleaseAnyway:true});
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toMatchObject({contentMode:'specific',requestedVolumes:[4],requestedEdition:'master',selectedReleaseId:'release',selectedArchive:{path:'Master/Alternative v04.cbz',size:400},useReleaseAnyway:true});
  } finally { vi.unstubAllGlobals(); }
});

it('the shared request carries a concrete multi-file selection independently of catalog units', async () => {
 const {planByTitle}=await import('./api');
 const fetchMock=vi.fn().mockResolvedValue(new Response('{}',{status:200}));vi.stubGlobal('fetch',fetchMock);
 try {const selectedArchives=[{path:'release/book.zip',size:123},{path:'release/other.zip',size:456}];await planByTitle({title:'Unknown edition',selectedReleaseId:'source',selectedArchives,useReleaseAnyway:true});expect(JSON.parse(fetchMock.mock.calls[0][1].body).selectedArchives).toEqual(selectedArchives);}finally{vi.unstubAllGlobals();}
});
