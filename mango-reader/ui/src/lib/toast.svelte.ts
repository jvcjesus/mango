export type ToastKind = 'success' | 'error' | 'info';

export type Toast = {
  id: string;
  text: string;
  kind: ToastKind;
  action?: { label: string; run: () => void };
  created: number;
  duration: number;
};

export type ToastOptions = {
  action?: { label: string; run: () => void };
  duration?: number;
};

class ToastStore {
  toasts = $state<Toast[]>([]);

  private parseOptions(
    opt?: { label: string; run: () => void } | ToastOptions,
  ): ToastOptions {
    if (!opt) return {};
    if ('label' in opt && typeof opt.label === 'string' && 'run' in opt) {
      return { action: opt as { label: string; run: () => void } };
    }
    return opt as ToastOptions;
  }

  show(
    text: string,
    options: {
      kind?: ToastKind;
      action?: { label: string; run: () => void };
      duration?: number;
    } = {},
  ): string {
    const id = Math.random().toString(36).slice(2, 9);
    const kind = options.kind ?? 'info';
    const duration = options.duration ?? (kind === 'error' ? 6000 : 4000);
    const item: Toast = {
      id,
      text,
      kind,
      action: options.action,
      created: Date.now(),
      duration,
    };
    // Keep max 4 toasts visible at a time to prevent screen clutter
    this.toasts = [...this.toasts.slice(-3), item];
    if (duration > 0) {
      setTimeout(() => this.dismiss(id), duration);
    }
    return id;
  }

  success(text: string, actionOrOptions?: { label: string; run: () => void } | ToastOptions) {
    const opts = this.parseOptions(actionOrOptions);
    return this.show(text, { kind: 'success', ...opts });
  }

  error(text: string, actionOrOptions?: { label: string; run: () => void } | ToastOptions) {
    const opts = this.parseOptions(actionOrOptions);
    return this.show(text, { kind: 'error', ...opts });
  }

  info(text: string, actionOrOptions?: { label: string; run: () => void } | ToastOptions) {
    const opts = this.parseOptions(actionOrOptions);
    return this.show(text, { kind: 'info', ...opts });
  }

  dismiss(id: string) {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  clear() {
    this.toasts = [];
  }
}

export const toast = new ToastStore();
