export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected';
export type ReadyCheckStatus = 'idle' | 'searching' | 'ready' | 'accepting' | 'accepted' | 'error';
export type NotificationEvent = 'queuePopped' | 'autoAccepted' | 'gameStarted';

export interface WebhookResult {
  ok: boolean;
  message: string;
}

export interface NotificationEventSettings {
  queuePopped: boolean;
  autoAccepted: boolean;
  gameStarted: boolean;
}

export interface NotificationMessageSettings {
  queuePopped: string;
  autoAccepted: string;
  gameStarted: string;
}

export interface DiscordMention {
  id: string;
  nickname: string;
}

export interface AppSettings {
  schemaVersion: 2;
  autoAcceptEnabled: boolean;
  discordNotificationsEnabled: boolean;
  discordWebhookUrl: string;
  discordMentions: DiscordMention[];
  notificationEvents: NotificationEventSettings;
  notificationMessages: NotificationMessageSettings;
  minimizeToTray: boolean;
}

export interface AppCapabilities {
  lcuConnection: boolean;
  autoAccept: boolean;
  discordDelivery: boolean;
}

export interface AppState {
  settings: AppSettings;
  connectionStatus: ConnectionStatus;
  connectionMessage: string;
  readyCheckStatus: ReadyCheckStatus;
  readyCheckMessage: string;
  capabilities: AppCapabilities;
}

export type SettingsPatch = Partial<Omit<AppSettings, 'schemaVersion' | 'notificationEvents' | 'notificationMessages'>> & {
  notificationEvents?: Partial<NotificationEventSettings>;
  notificationMessages?: Partial<NotificationMessageSettings>;
};

export interface LeagueAutoAcceptBridge {
  getState(): Promise<AppState>;
  updateSettings(patch: SettingsPatch): Promise<AppState>;
  testWebhook(): Promise<WebhookResult>;
  onStateChanged(callback: (state: AppState) => void): () => void;
}

export const IPC_CHANNELS = {
  getState: 'laa:state:get',
  updateSettings: 'laa:settings:update',
  testWebhook: 'laa:discord:test',
  stateChanged: 'laa:state:changed'
} as const;
