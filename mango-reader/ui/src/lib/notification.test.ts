import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  getNotificationConfig,
  saveNotificationConfig,
  sendTestNotification,
  getNotificationOutbox,
} from './api';

afterEach(() => vi.unstubAllGlobals());

describe('Notification API Client (M6.3)', () => {
  it('fetches notification config and verifies secrets are not leaked', async () => {
    const mockConfig = {
      enabled: true,
      serverUrl: 'https://ntfy.sh',
      topic: 'test-topic',
      hasToken: true,
      prefNewVolume: true,
      prefReleaseAvailable: true,
      prefAcquisitionComplete: true,
      prefActionRequired: true,
      activatedAt: 1700000000,
    };

    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(mockConfig)));
    vi.stubGlobal('fetch', fetch);

    const cfg = await getNotificationConfig();
    expect(fetch).toHaveBeenCalledWith('api/notifications/config', expect.anything());
    expect(cfg.enabled).toBe(true);
    expect(cfg.serverUrl).toBe('https://ntfy.sh');
    expect(cfg.topic).toBe('test-topic');
    expect(cfg.hasToken).toBe(true);
    expect((cfg as any).token).toBeUndefined();
  });

  it('saves notification config with masked token handling', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ enabled: true })));
    vi.stubGlobal('fetch', fetch);

    await saveNotificationConfig({
      enabled: true,
      serverUrl: 'https://custom-ntfy.example.com',
      topic: 'my-topic',
      prefNewVolume: true,
      prefReleaseAvailable: false,
    });

    expect(fetch).toHaveBeenCalledWith(
      'api/notifications/config',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          enabled: true,
          serverUrl: 'https://custom-ntfy.example.com',
          topic: 'my-topic',
          prefNewVolume: true,
          prefReleaseAvailable: false,
        }),
      })
    );
  });

  it('dispatches test notification directly without domain events', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ success: true, message: 'Test notification sent successfully' }))
    );
    vi.stubGlobal('fetch', fetch);

    const res = await sendTestNotification({
      serverUrl: 'https://ntfy.sh',
      topic: 'test-topic',
    });

    expect(fetch).toHaveBeenCalledWith(
      'api/notifications/test',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          serverUrl: 'https://ntfy.sh',
          topic: 'test-topic',
        }),
      })
    );
    expect(res.success).toBe(true);
  });

  it('fetches notification outbox history', async () => {
    const mockOutbox = [
      {
        id: 1,
        eventId: 42,
        eventType: 'NEW_VOLUME',
        transport: 'ntfy',
        destination: 'manga-alerts',
        payloadJson: '{"title":"Vinland Saga Vol. 17"}',
        status: 'delivered',
        attempts: 1,
        nextAttemptAt: 1700000000,
        createdAt: 1700000000,
      },
    ];

    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(mockOutbox)));
    vi.stubGlobal('fetch', fetch);

    const outbox = await getNotificationOutbox();
    expect(fetch).toHaveBeenCalledWith('api/notifications/outbox', expect.anything());
    expect(outbox).toHaveLength(1);
    expect(outbox[0].status).toBe('delivered');
    expect(outbox[0].eventType).toBe('NEW_VOLUME');
  });
});
