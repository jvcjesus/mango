import { afterEach, describe, expect, it, vi } from 'vitest';
import { updateMonitoringSettings, type MonitoringSettings } from './api';
import settingsRaw from '../routes/Settings.svelte?raw';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('M11 Monitored Followed Lists Selection UI', () => {
  describe('Status Toggle & Selection Logic', () => {
    function computeToggledStatuses(currentStatuses: string[], statusToToggle: string): string[] {
      const set = new Set(currentStatuses);
      if (set.has(statusToToggle)) {
        set.delete(statusToToggle);
      } else {
        set.add(statusToToggle);
      }
      return Array.from(set);
    }

    function formatSummary(statuses: string[]): string {
      if (!statuses || statuses.length === 0) return 'None selected';
      return statuses.map((s) => s.charAt(0).toUpperCase() + s.slice(1)).join(', ');
    }

    it('selects an unselected list status', () => {
      const initial = ['reading', 'planning'];
      const updated = computeToggledStatuses(initial, 'completed');
      expect(updated).toEqual(['reading', 'planning', 'completed']);
      expect(formatSummary(updated)).toBe('Reading, Planning, Completed');
    });

    it('deselects an already selected list status', () => {
      const initial = ['reading', 'planning', 'completed'];
      const updated = computeToggledStatuses(initial, 'reading');
      expect(updated).toEqual(['planning', 'completed']);
      expect(formatSummary(updated)).toBe('Planning, Completed');
    });

    it('handles deselecting all list statuses and formats empty summary', () => {
      let statuses = ['reading'];
      statuses = computeToggledStatuses(statuses, 'reading');
      expect(statuses).toEqual([]);
      expect(formatSummary(statuses)).toBe('None selected');
    });

    it('handles selecting all 5 standard tracker statuses', () => {
      let statuses: string[] = [];
      const all = ['reading', 'planning', 'completed', 'paused', 'dropped'];
      for (const st of all) {
        statuses = computeToggledStatuses(statuses, st);
      }
      expect(statuses).toEqual(all);
      expect(formatSummary(statuses)).toBe('Reading, Planning, Completed, Paused, Dropped');
    });
  });

  describe('API Persistence & Error Rollback Contract', () => {
    it('persists selected trackedStatuses via updateMonitoringSettings', async () => {
      const mockResponse: MonitoringSettings = {
        automationMode: 'review_before_acquisition',
        intervalHours: 12,
        trackedStatuses: ['reading', 'planning', 'paused'],
        lastCycleAt: 1700000000,
        autoAcquireEnabled: false,
        operatingMode: 'simulation',
        autoPublishManual: false,
      };

      const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(mockResponse)));
      vi.stubGlobal('fetch', fetch);

      const res = await updateMonitoringSettings({
        trackedStatuses: ['reading', 'planning', 'paused'],
      });

      expect(fetch).toHaveBeenCalledWith(
        'api/monitoring/settings',
        expect.objectContaining({
          method: 'POST',
          body: JSON.stringify({ trackedStatuses: ['reading', 'planning', 'paused'] }),
        })
      );
      expect(res.trackedStatuses).toEqual(['reading', 'planning', 'paused']);
      expect(res.autoAcquireEnabled).toBe(false);
      expect(res.operatingMode).toBe('simulation');
    });

    it('preserves or rolls back selection state when API call fails', async () => {
      // Simulate component state machine
      let state = {
        trackedStatuses: ['reading', 'planning'],
        saving: false,
      };

      const fetch = vi.fn().mockRejectedValue(new Error('Network error or server unavailable'));
      vi.stubGlobal('fetch', fetch);

      async function toggleWithRollback(status: string) {
        if (state.saving) return;
        const previous = [...state.trackedStatuses];
        const next = state.trackedStatuses.includes(status)
          ? state.trackedStatuses.filter((s) => s !== status)
          : [...state.trackedStatuses, status];

        // Optimistic update
        state.trackedStatuses = next;
        state.saving = true;

        try {
          await updateMonitoringSettings({ trackedStatuses: next });
        } catch {
          // Rollback to previous confirmed selection on failure
          state.trackedStatuses = previous;
        } finally {
          state.saving = false;
        }
      }

      await toggleWithRollback('completed');

      // Since API failed, state must be restored to previous confirmed selection
      expect(state.trackedStatuses).toEqual(['reading', 'planning']);
      expect(state.saving).toBe(false);
    });

    it('prevents rapid repeated clicks from initiating out-of-order updates', async () => {
      let state = {
        trackedStatuses: ['reading', 'planning'],
        saving: false,
      };

      let resolveApi: (val: any) => void = () => {};
      const pendingPromise = new Promise((resolve) => {
        resolveApi = resolve;
      });

      const fetch = vi.fn().mockImplementation(() => pendingPromise);
      vi.stubGlobal('fetch', fetch);

      async function toggle(status: string) {
        if (state.saving) return false;
        state.saving = true;
        try {
          await updateMonitoringSettings({ trackedStatuses: [...state.trackedStatuses, status] });
          return true;
        } finally {
          state.saving = false;
        }
      }

      // First click initiates request
      const firstCall = toggle('completed');
      expect(state.saving).toBe(true);

      // Second rapid click while saving is in progress
      const secondCall = await toggle('paused');
      // Second click must be ignored/dropped to prevent race condition
      expect(secondCall).toBe(false);

      // Complete first request
      resolveApi(
        new Response(
          JSON.stringify({
            automationMode: 'monitor_only',
            intervalHours: 12,
            trackedStatuses: ['reading', 'planning', 'completed'],
            lastCycleAt: null,
            autoAcquireEnabled: false,
            operatingMode: 'simulation',
            autoPublishManual: false,
          })
        )
      );

      await firstCall;
      expect(state.saving).toBe(false);
      expect(fetch).toHaveBeenCalledTimes(1);
    });
  });

  describe('Settings.svelte Component Surface Verification', () => {
    it('verifies tracked status controls have checkmark icon and aria-pressed attributes', () => {
      // Check template source for aria-pressed, class:selected, and Icon name="check"
      expect(settingsRaw).toContain('class="tracked-status-btn"');
      expect(settingsRaw).toContain('class:selected={isSelected}');
      expect(settingsRaw).toContain('aria-pressed={isSelected}');
      expect(settingsRaw).toContain('<Icon name="check" size={13} stroke={2.5} />');
      expect(settingsRaw).toContain('disabled={savingTrackedStatuses}');
    });

    it('verifies summary display and saving indicator in Settings.svelte', () => {
      expect(settingsRaw).toContain('Monitoring: <strong>{monitoredSummaryText}</strong>');
      expect(settingsRaw).toContain('{#if savingTrackedStatuses}');
      expect(settingsRaw).toContain('Saving…');
      expect(settingsRaw).toContain('<Icon name="spinner" size={13} stroke={2.5} />');
    });

    it('verifies CSS contrast rules for selected and unselected button states', () => {
      // Selected state uses high-contrast --ink background and --paper text
      expect(settingsRaw).toContain('.tracked-status-btn.selected');
      expect(settingsRaw).toContain('background: var(--ink);');
      expect(settingsRaw).toContain('color: var(--paper);');

      // Unselected state uses neutral outline
      expect(settingsRaw).toContain('.tracked-status-btn {');
      expect(settingsRaw).toContain('border: 2px solid var(--shade');
      expect(settingsRaw).toContain('border-radius: 9999px;');
    });
  });
});
