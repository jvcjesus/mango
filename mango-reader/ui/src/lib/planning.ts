export const PLANNING_TIMEOUT_MS = 35_000;
export const SOURCE_OPTIONS_TIMEOUT_MS = 10_000;
export type PlanningResult<T> = { status: 'ready'; value: T } | { status: 'failed'; error: Error } | { status: 'stale' };
export class PlanningSession {
  private generation = 0;
  private controller?: AbortController;
  cancel() { this.generation++; this.controller?.abort(); this.controller = undefined; }
  async run<T>(operation: (signal: AbortSignal) => Promise<T>, timeout = PLANNING_TIMEOUT_MS): Promise<PlanningResult<T>> {
    this.cancel();
    const generation = this.generation;
    const controller = this.controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const aborted = new Promise<never>((_, reject) => {
      controller.signal.addEventListener('abort', () => reject(new Error('Discovery canceled.')), { once: true });
      timer = setTimeout(() => { reject(new Error('Release discovery timed out. Try Again or choose another source.')); controller.abort(); }, timeout);
    });
    // Keep cancellation handled even if an operation throws synchronously.
    void aborted.catch(() => {});
    try {
      const value = await Promise.race([operation(controller.signal), aborted]);
      return generation === this.generation ? { status: 'ready', value } : { status: 'stale' };
    } catch (error) {
      return generation === this.generation ? { status: 'failed', error: error instanceof Error ? error : new Error(String(error)) } : { status: 'stale' };
    } finally { clearTimeout(timer); }
  }
}
export function parseRequestedVolumes(value: string): number[] {
  const volumes = new Set<number>();
  for (const part of value.split(',')) {
    const text = part.trim(); if (!text) continue;
    const range = text.split('-').map(Number);
    if (range.some(n => !Number.isFinite(n) || n < 0) || range.length > 2) throw new Error('Enter valid edition-local volumes, for example 1, 4-6.');
    if (range.length === 2) {
      const [start, end] = range;
      if (end < start || end - start > 499 || !Number.isInteger(start) || !Number.isInteger(end)) throw new Error('Choose a range of at most 500 volumes.');
      for (let n = start; n <= end; n++) volumes.add(n);
    } else volumes.add(range[0]);
    if (volumes.size > 500) throw new Error('Choose at most 500 volumes.');
  }
  if (!volumes.size) throw new Error('Specify the edition-local volumes to acquire.');
  return [...volumes].sort((a,b) => a-b);
}
