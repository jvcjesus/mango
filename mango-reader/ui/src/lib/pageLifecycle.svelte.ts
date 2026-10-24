export type PageStatus = 'idle' | 'loading' | 'loaded' | 'retrying' | 'failed';
export type PageErrorType = 'network' | 'decode' | 'timeout' | null;

export const MAX_AUTO_RETRIES = 2;
export const PAGE_TIMEOUT_MS = 25_000;

export interface PageDiagnostics {
  index: number;
  status: PageStatus;
  retryCount: number;
  error: string | null;
  errorType: PageErrorType;
  startTime: number;
  durationMs: number | null;
  naturalWidth?: number;
  naturalHeight?: number;
  attempt: number;
}

export class PageState {
  index: number;
  status = $state<PageStatus>('idle');
  retryCount = $state<number>(0);
  error = $state<string | null>(null);
  errorType = $state<PageErrorType>(null);
  startTime = $state<number>(0);
  durationMs = $state<number | null>(null);
  naturalWidth = $state<number | undefined>(undefined);
  naturalHeight = $state<number | undefined>(undefined);
  attempt = $state<number>(0);

  private timeoutTimer: ReturnType<typeof setTimeout> | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private isDestroyed = false;

  constructor(index: number) {
    this.index = index;
  }

  /** Starts or restarts a load attempt. */
  startLoad() {
    if (this.isDestroyed) return;
    this.clearTimers();
    this.status = this.retryCount > 0 ? 'retrying' : 'loading';
    this.startTime = Date.now();
    this.error = null;
    this.errorType = null;

    // Timeout protection against permanently stalled requests
    this.timeoutTimer = setTimeout(() => {
      this.handleTimeout();
    }, PAGE_TIMEOUT_MS);
  }

  /** Called when <img> fires onload. Verifies decoding via decode() if available. */
  async handleLoad(img: HTMLImageElement): Promise<boolean> {
    if (this.isDestroyed) return false;
    this.clearTimers();

    // Verify image decoding
    if (typeof img.decode === 'function') {
      try {
        await img.decode();
      } catch (e) {
        this.handleFailure('Image decode failed: corrupt or unsupported image data', 'decode');
        return false;
      }
    }

    if (this.isDestroyed) return false;

    // Zero-dimension verification
    if (img.naturalWidth === 0 && img.naturalHeight === 0) {
      this.handleFailure('Image decoded with zero dimensions', 'decode');
      return false;
    }

    this.status = 'loaded';
    this.durationMs = Date.now() - this.startTime;
    this.naturalWidth = img.naturalWidth;
    this.naturalHeight = img.naturalHeight;
    this.error = null;
    this.errorType = null;
    return true;
  }

  /** Handles network or decode failure and triggers bounded retry if eligible. */
  handleFailure(errorMsg: string, type: PageErrorType = 'network', onAutoRetry?: () => void) {
    if (this.isDestroyed) return;
    this.clearTimers();
    this.error = errorMsg;
    this.errorType = type;

    if (this.retryCount < MAX_AUTO_RETRIES) {
      this.retryCount += 1;
      this.attempt += 1;
      this.status = 'retrying';

      // Exponential backoff with jitter to prevent storming
      const baseDelay = this.retryCount === 1 ? 800 : 2000;
      const jitter = Math.floor(Math.random() * 250);
      const delay = baseDelay + jitter;

      this.retryTimer = setTimeout(() => {
        if (!this.isDestroyed) {
          this.startLoad();
          onAutoRetry?.();
        }
      }, delay);
    } else {
      this.status = 'failed';
    }
  }

  private handleTimeout() {
    this.handleFailure(`Image request timed out after ${PAGE_TIMEOUT_MS / 1000}s`, 'timeout');
  }

  /** Manual user retry: resets retry count and forces a fresh request. */
  manualRetry(onExecute?: () => void) {
    if (this.isDestroyed || this.status === 'loading' || this.status === 'retrying') return;
    this.clearTimers();
    this.retryCount = 0;
    this.attempt += 1;
    this.error = null;
    this.errorType = null;
    this.startLoad();
    onExecute?.();
  }

  get diagnostics(): PageDiagnostics {
    return {
      index: this.index,
      status: this.status,
      retryCount: this.retryCount,
      error: this.error,
      errorType: this.errorType,
      startTime: this.startTime,
      durationMs: this.durationMs,
      naturalWidth: this.naturalWidth,
      naturalHeight: this.naturalHeight,
      attempt: this.attempt,
    };
  }

  private clearTimers() {
    if (this.timeoutTimer) {
      clearTimeout(this.timeoutTimer);
      this.timeoutTimer = null;
    }
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
  }

  destroy() {
    this.isDestroyed = true;
    this.clearTimers();
  }
}

export class PageLifecycleManager {
  private pages = new Map<number, PageState>();

  get(index: number): PageState {
    let page = this.pages.get(index);
    if (!page) {
      page = new PageState(index);
      this.pages.set(index, page);
    }
    return page;
  }

  isLoaded(index: number): boolean {
    return this.pages.get(index)?.status === 'loaded';
  }

  isFailed(index: number): boolean {
    return this.pages.get(index)?.status === 'failed';
  }

  isErrorOrRetrying(index: number): boolean {
    const status = this.pages.get(index)?.status;
    return status === 'failed' || status === 'retrying';
  }

  /** Safe progress check: returns true only if count is valid and final page loaded. */
  canMarkRead(totalCount: number): boolean {
    if (totalCount <= 0) return false;
    const finalPage = this.pages.get(totalCount - 1);
    return finalPage?.status === 'loaded';
  }

  destroy() {
    for (const page of this.pages.values()) {
      page.destroy();
    }
    this.pages.clear();
  }
}
