export type ConfirmOptions = {
  title?: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  danger?: boolean;
};

type PendingConfirm = ConfirmOptions & {
  resolve: (value: boolean) => void;
};

class ConfirmStore {
  current = $state<PendingConfirm | null>(null);

  ask(options: ConfirmOptions): Promise<boolean> {
    return new Promise((resolve) => {
      this.current = {
        title: options.title ?? 'Are you sure?',
        message: options.message,
        confirmText: options.confirmText ?? 'Confirm',
        cancelText: options.cancelText ?? 'Cancel',
        danger: options.danger ?? false,
        resolve: (result: boolean) => {
          this.current = null;
          resolve(result);
        },
      };
    });
  }
}

export const confirmation = new ConfirmStore();

export const confirm = (options: ConfirmOptions | string): Promise<boolean> => {
  if (typeof options === 'string') {
    return confirmation.ask({ message: options });
  }
  return confirmation.ask(options);
};
