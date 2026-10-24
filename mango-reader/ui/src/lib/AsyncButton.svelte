<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon, { type IconName } from './Icon.svelte';

  let {
    loading = false,
    disabled = false,
    onclick,
    class: className = 'btn',
    type = 'button',
    ariaLabel,
    ariaPressed,
    title,
    icon,
    iconSize = 18,
    iconFill = false,
    iconStroke = 2.5,
    children,
  }: {
    loading?: boolean;
    disabled?: boolean;
    onclick?: (event: MouseEvent) => unknown | Promise<unknown>;
    class?: string;
    type?: 'button' | 'submit' | 'reset';
    ariaLabel?: string;
    ariaPressed?: boolean;
    title?: string;
    icon?: IconName;
    iconSize?: number;
    iconFill?: boolean;
    iconStroke?: number;
    children?: Snippet;
  } = $props();

  let busy = $state(false);
  const active = $derived(loading || busy);
  const effectivelyDisabled = $derived(disabled || active);

  async function handleClick(event: MouseEvent) {
    if (effectivelyDisabled || !onclick) return;
    try {
      const result = onclick(event);
      if (result && typeof (result as Promise<unknown>).then === 'function') {
        busy = true;
        await result;
      }
    } finally {
      busy = false;
    }
  }
</script>

<button
  {type}
  class={className}
  disabled={effectivelyDisabled}
  aria-disabled={effectivelyDisabled ? 'true' : undefined}
  aria-busy={active ? 'true' : undefined}
  aria-label={ariaLabel}
  aria-pressed={ariaPressed}
  {title}
  onclick={handleClick}
>
  {#if active}
    <span class="spin icon-slot" aria-hidden="true">
      <Icon name="spinner" size={iconSize} stroke={3} />
    </span>
  {:else if icon}
    <span class="icon-slot" aria-hidden="true">
      <Icon name={icon} size={iconSize} fill={iconFill} stroke={iconStroke} />
    </span>
  {/if}
  {#if children}
    {@render children()}
  {/if}
</button>

<style>
  .icon-slot {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
  }
</style>
