import type {AppSettings, NotificationEvent, WebhookResult} from '../shared/contracts';
import type {DiscordWebhookSender} from './discord-webhook-client';

export class DiscordNotificationService {
  constructor(
    private readonly getSettings: () => AppSettings,
    private readonly sender: DiscordWebhookSender
  ) {
  }

  public notify(event: NotificationEvent): void {
    const settings = this.getSettings();
    if (!settings.discordNotificationsEnabled || !settings.notificationEvents[event]) return;
    const userIds = settings.discordMentions.map(mention => mention.id);
    const content = renderNotificationMessage(settings.notificationMessages[event], userIds);
    void this.sender.send(settings.discordWebhookUrl, content, userIds).catch(() => undefined);
  }

  public test(): Promise<WebhookResult> {
    const settings = this.getSettings();
    const userIds = settings.discordMentions.map(mention => mention.id);
    const mentions = userIds.length ? '{mentions} ' : '';
    const content = renderNotificationMessage(`${mentions}League Auto Accept webhook test.`, userIds);
    return this.sender.send(settings.discordWebhookUrl, content, userIds)
      .catch(() => ({ok: false, message: 'Discord webhook request failed.'}));
  }
}

export function renderNotificationMessage(template: string, userIds: string[]): string {
  const mentions = userIds.map(id => `<@${id}>`).join(' ');
  return template.replaceAll('{mentions}', mentions).trim();
}
