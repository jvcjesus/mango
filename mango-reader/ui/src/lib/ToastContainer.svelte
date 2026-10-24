<script lang="ts">
  import Icon from './Icon.svelte';
  import { toast, type Toast } from './toast.svelte';

  const items = $derived(toast.toasts);
</script>

{#if items.length}
  <aside class="toasts" aria-label="Notifications" role="region">
    {#each items as item (item.id)}
      <div
        class="toast panel"
        class:success={item.kind === 'success'}
        class:error={item.kind === 'error'}
        role={item.kind === 'error' ? 'alert' : 'status'}
      >
        <span class="mark" aria-hidden="true">
          {#if item.kind === 'success'}
            <Icon name="check" size={16} stroke={3.5} />
          {:else if item.kind === 'error'}
            <Icon name="info" size={16} stroke={3} />
          {:else}
            <Icon name="info" size={16} stroke={2.5} />
          {/if}
        </span>
        <span class="text">{item.text}</span>
        {#if item.action}
          <button
            class="btn action"
            onclick={() => {
              item.action?.run();
              toast.dismiss(item.id);
            }}
          >
            {item.action.label}
          </button>
        {/if}
        <button
          class="btn icon close"
          aria-label="Dismiss notification"
          onclick={() => toast.dismiss(item.id)}
        >
          <Icon name="close" size={14} stroke={3} />
        </button>
      </div>
    {/each}
  </aside>
{/if}

<style>
  .toasts {
    position: fixed;
    right: max(16px, var(--safe-right));
    bottom: max(16px, calc(var(--tabs) + 12px));
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-width: min(420px, calc(100vw - 32px));
    pointer-events: none;
  }

  .toast {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border: 3px solid var(--ink);
    background: var(--surface);
    box-shadow: var(--sh-4);
    pointer-events: auto;
    animation: toast-in 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  }

  @keyframes toast-in {
    from {
      transform: translateY(12px);
      opacity: 0;
    }
    to {
      transform: translateY(0);
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .toast {
      animation: none;
    }
  }

  .mark {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: 2px solid var(--ink);
    border-radius: 50%;
    background: var(--paper);
    color: var(--ink);
  }

  .toast.success .mark {
    background: var(--spot);
    color: var(--on-spot);
  }

  .toast.error .mark {
    background: var(--ink);
    color: var(--paper);
  }

  .text {
    flex: 1 1 auto;
    min-width: 0;
    font-size: 13.5px;
    font-weight: 700;
    line-height: 1.35;
    overflow-wrap: anywhere;
  }

  .action {
    flex: none;
    min-height: 32px;
    padding: 0 10px;
    font-size: 12px;
  }

  .close {
    flex: none;
    width: 32px;
    min-height: 32px;
    border-width: 2px;
  }
</style>
