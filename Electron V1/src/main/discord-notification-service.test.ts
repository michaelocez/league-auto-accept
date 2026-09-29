import assert from 'node:assert/strict';
import {describe, it} from 'node:test';
import type {AppSettings, WebhookResult} from '../shared/contracts';
import {DEFAULT_SETTINGS} from '../shared/settings';
import {DiscordNotificationService, renderNotificationMessage} from './discord-notification-service';
import type {DiscordWebhookSender} from './discord-webhook-client';

class FakeSender implements DiscordWebhookSender {
  public calls: Array<{url: string; content: string; ids: string[]}> = [];
  public reject = false;

  public async send(url: string, content: string, ids: string[]): Promise<WebhookResult> {
    this.calls.push({url, content, ids});
    if (this.reject) throw new Error('offline');
    return {ok: true, message: 'Discord webhook sent.'};
  }
}

function settings(): AppSettings {
  return {
    ...structuredClone(DEFAULT_SETTINGS),
    discordNotificationsEnabled: true,
    discordWebhookUrl: `https://discord.com/api/webhooks/${'1'.repeat(17)}/${'token'.repeat(8)}`,
    discordMentions: [
      {id: '12345678901234567', nickname: 'Michael'},
      {id: '23456789012345678', nickname: 'Jungler'}
    ],
    notificationEvents: {queuePopped: true, autoAccepted: true, gameStarted: true}
  };
}

describe('DiscordNotificationService', () => {
  it('renders all configured mentions wherever the placeholder appears', () => {
    const rendered = renderNotificationMessage('{mentions} accepted for {mentions}', [
      '12345678901234567',
      '23456789012345678'
    ]);
    assert.equal(
      rendered,
      '<@12345678901234567> <@23456789012345678> accepted for <@12345678901234567> <@23456789012345678>'
    );
  });

  it('sends enabled events with the editable template and explicit mention allowlist', async () => {
    const current = settings();
    current.notificationMessages.queuePopped = '{mentions} custom queue message';
    const sender = new FakeSender();
    const service = new DiscordNotificationService(() => current, sender);
    service.notify('queuePopped');
    await Promise.resolve();

    assert.equal(sender.calls.length, 1);
    assert.equal(sender.calls[0]?.content, '<@12345678901234567> <@23456789012345678> custom queue message');
    assert.deepEqual(sender.calls[0]?.ids, current.discordMentions.map(mention => mention.id));
  });

  it('skips globally disabled and individually disabled events', async () => {
    const current = settings();
    const sender = new FakeSender();
    const service = new DiscordNotificationService(() => current, sender);
    current.discordNotificationsEnabled = false;
    service.notify('queuePopped');
    current.discordNotificationsEnabled = true;
    current.notificationEvents.queuePopped = false;
    service.notify('queuePopped');
    await Promise.resolve();

    assert.equal(sender.calls.length, 0);
  });

  it('contains rejected delivery promises and returns test failures as results', async () => {
    const current = settings();
    const sender = new FakeSender();
    sender.reject = true;
    const service = new DiscordNotificationService(() => current, sender);
    service.notify('autoAccepted');
    await Promise.resolve();
    const result = await service.test();

    assert.equal(sender.calls.length, 2);
    assert.deepEqual(result, {ok: false, message: 'Discord webhook request failed.'});
  });
});
