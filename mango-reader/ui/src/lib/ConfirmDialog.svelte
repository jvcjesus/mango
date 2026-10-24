<script lang="ts">
  import Icon from './Icon.svelte';
  import { confirmation } from './confirm.svelte';

  const current = $derived(confirmation.current);

  function keydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && current) {
      event.preventDefault();
      current.resolve(false);
    }
  }

  function backdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget && current) {
      current.resolve(false);
    }
  }
</script>

<svelte:window onkeydown={keydown} />

{#if current}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <div
    class="backdrop"
    onclick={backdropClick}
    role="presentation"
  >
    <div
      class="panel dialog"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="confirm-title"
      aria-describedby="confirm-desc"
    >
      <div class="head">
        <h2 id="confirm-title" class="display">{current.title}</h2>
        <button
          class="btn icon close"
          aria-label="Cancel"
          onclick={() => current?.resolve(false)}
        >
          <Icon name="close" size={16} stroke={3} />
        </button>
      </div>

      <p id="confirm-desc" class="desc">{current.message}</p>

      <div class="actions">
        <button
          class="btn big"
          onclick={() => current?.resolve(false)}
        >
          {current.cancelText}
        </button>
        <button
          class="btn big"
          class:ink={current.danger}
          class:spot={!current.danger}
          onclick={() => current?.resolve(true)}
        >
          {#if current.danger}
            <Icon name="trash" size={18} stroke={3} />
          {:else}
            <Icon name="check" size={18} stroke={3} />
          {/if}
          {current.confirmText}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 80;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
    background: rgba(0, 0, 0, 0.45);
    animation: fade-in 0.15s ease-out;
  }

  @keyframes fade-in {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  .dialog {
    display: flex;
    flex-direction: column;
    gap: 16px;
    width: 100%;
    max-width: 440px;
    padding: 24px;
    border: 3px solid var(--ink);
    background: var(--surface);
    box-shadow: var(--sh-8);
    animation: pop-in 0.15s cubic-bezier(0.16, 1, 0.3, 1);
  }

  @keyframes pop-in {
    from {
      transform: scale(0.96);
      opacity: 0;
    }
    to {
      transform: scale(1);
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .backdrop,
    .dialog {
      animation: none;
    }
  }

  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .head h2 {
    font-size: 22px;
    line-height: 1.15;
    overflow-wrap: anywhere;
  }

  .close {
    flex: none;
    width: 36px;
    min-height: 36px;
  }

  .desc {
    font-size: 15px;
    font-weight: 700;
    line-height: 1.45;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 12px;
    margin-top: 8px;
  }

  .actions button {
    flex: 1 1 120px;
  }
</style>
