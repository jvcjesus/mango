import { message } from './api';
import { toast } from './toast.svelte';

export type ActionOptions<T> = {
  success?: string | ((result: T) => string);
  error?: string | ((err: unknown) => string);
  onError?: (err: unknown) => void;
};

export async function runAction<T>(
  work: () => Promise<T>,
  options: ActionOptions<T> = {},
): Promise<T | null> {
  try {
    const result = await work();
    if (options.success) {
      const msg = typeof options.success === 'function' ? options.success(result) : options.success;
      if (msg) toast.success(msg);
    }
    return result;
  } catch (err) {
    const msg = options.error
      ? typeof options.error === 'function'
        ? options.error(err)
        : options.error
      : message(err);
    if (msg) toast.error(msg);
    options.onError?.(err);
    return null;
  }
}
