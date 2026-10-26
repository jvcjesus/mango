import { describe, expect, it } from 'vitest';
import { compile } from 'svelte/compiler';
import lists from '../routes/Lists.svelte?raw';

// Check the component's compiled CSS contract, rather than snapshotting page markup.
// The regression was a class mismatch: the planner never received its modal styles.
const compiled = compile(lists, { filename: 'Lists.svelte', generate: 'client' });
const css = compiled.css?.code ?? '';
const rule = (selector: string) => {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return css.match(new RegExp(`${escaped}(?:\\.[\\w-]+)?\\s*\\{([^}]+)\\}`))?.[1] ?? '';
};

describe('Lists acquisition overlay surface', () => {
  it('connects the planner to the opaque, bounded modal surface', () => {
    expect(lists).toMatch(/class="modal planning-modal"/);
    expect(rule('.modal')).toContain('background: var(--paper)');
    expect(rule('.modal')).toContain('overflow-y: auto');
    expect(rule('.planning-modal')).toContain('background: var(--surface)');
    expect(rule('.planning-modal')).toContain('100dvh');
  });

  it('keeps source controls themed and constrained to the scrollable panel', () => {
    expect(rule('.source-select')).toContain('background: var(--surface)');
    expect(rule('.source-select')).toContain('color: var(--ink)');
    expect(rule('.source-select')).toContain('width: 100%');
    expect(rule('.modal-body-scroll')).toContain('overflow-y: auto');
    expect(rule('.segmented-control')).toContain('flex-wrap: wrap');
  });
});
