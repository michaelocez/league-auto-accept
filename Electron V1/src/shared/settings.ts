import type {AppSettings, DiscordMention, SettingsPatch} from './contracts';

export const MAX_DISCORD_USER_IDS = 5;
export const MAX_DISCORD_NICKNAME_LENGTH = 40;
export const MAX_WEBHOOK_URL_LENGTH = 2048;
export const MAX_NOTIFICATION_MESSAGE_LENGTH = 1800;

export const DEFAULT_SETTINGS: AppSettings = {
  schemaVersion: 2,
  autoAcceptEnabled: false,
  discordNotificationsEnabled: false,
  discordWebhookUrl: '',
  discordMentions: [],
  notificationEvents: {
    queuePopped: false,
    autoAccepted: true,
    gameStarted: false
  },
  notificationMessages: {
    queuePopped: '{mentions} your League queue has popped.',
    autoAccepted: '{mentions} the ready check was automatically accepted.',
    gameStarted: '{mentions} the game is now in progress.'
  },
  minimizeToTray: true
};

function objectValue(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function booleanValue(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function stringValue(value: unknown, fallback: string, maxLength: number): string {
  return typeof value === 'string' ? value.slice(0, maxLength) : fallback;
}

function discordMentions(value: unknown, legacyValue: unknown, fallback: DiscordMention[]): DiscordMention[] {
  const candidates = Array.isArray(value)
    ? value
    : Array.isArray(legacyValue)
      ? legacyValue.map(id => ({id, nickname: ''}))
      : fallback;
  const output: DiscordMention[] = [];
  for (const candidate of candidates) {
    const record = objectValue(candidate);
    const id = typeof record.id === 'string' ? record.id.trim() : '';
    const nickname = typeof record.nickname === 'string'
      ? record.nickname.trim().slice(0, MAX_DISCORD_NICKNAME_LENGTH)
      : '';
    if (!/^\d{17,20}$/.test(id) || output.some(item => item.id === id)) continue;
    output.push({id, nickname});
    if (output.length === MAX_DISCORD_USER_IDS) break;
  }
  return output;
}

export function normalizeSettings(value: unknown, fallback: AppSettings = DEFAULT_SETTINGS): AppSettings {
  const source = objectValue(value);
  const events = objectValue(source.notificationEvents);
  const messages = objectValue(source.notificationMessages);

  return {
    schemaVersion: 2,
    autoAcceptEnabled: booleanValue(source.autoAcceptEnabled, fallback.autoAcceptEnabled),
    discordNotificationsEnabled: booleanValue(
      source.discordNotificationsEnabled,
      fallback.discordNotificationsEnabled
    ),
    discordWebhookUrl: stringValue(source.discordWebhookUrl, fallback.discordWebhookUrl, MAX_WEBHOOK_URL_LENGTH).trim(),
    discordMentions: discordMentions(source.discordMentions, source.discordUserIds, fallback.discordMentions),
    notificationEvents: {
      queuePopped: booleanValue(events.queuePopped, fallback.notificationEvents.queuePopped),
      autoAccepted: booleanValue(events.autoAccepted, fallback.notificationEvents.autoAccepted),
      gameStarted: booleanValue(events.gameStarted, fallback.notificationEvents.gameStarted)
    },
    notificationMessages: {
      queuePopped: stringValue(
        messages.queuePopped,
        fallback.notificationMessages.queuePopped,
        MAX_NOTIFICATION_MESSAGE_LENGTH
      ),
      autoAccepted: stringValue(
        messages.autoAccepted,
        fallback.notificationMessages.autoAccepted,
        MAX_NOTIFICATION_MESSAGE_LENGTH
      ),
      gameStarted: stringValue(
        messages.gameStarted,
        fallback.notificationMessages.gameStarted,
        MAX_NOTIFICATION_MESSAGE_LENGTH
      )
    },
    minimizeToTray: booleanValue(source.minimizeToTray, fallback.minimizeToTray)
  };
}

export function mergeSettings(current: AppSettings, patch: unknown): AppSettings {
  const source = objectValue(patch) as SettingsPatch;
  const candidate = {
    ...current,
    ...source,
    schemaVersion: 2,
    notificationEvents: {
      ...current.notificationEvents,
      ...objectValue(source.notificationEvents)
    },
    notificationMessages: {
      ...current.notificationMessages,
      ...objectValue(source.notificationMessages)
    }
  };
  return normalizeSettings(candidate, current);
}
