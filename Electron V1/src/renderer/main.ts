import './styles.css';
import type {AppSettings, AppState, DiscordMention, SettingsPatch} from '../shared/contracts';
import {MAX_DISCORD_NICKNAME_LENGTH, MAX_DISCORD_USER_IDS} from '../shared/settings';

function element<T extends HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!value) throw new Error(`Missing required element: ${id}`);
  return value as T;
}

const controls = {
  connectionBadge: element<HTMLDivElement>('connectionBadge'),
  connectionLabel: element<HTMLSpanElement>('connectionLabel'),
  connectionMessage: element<HTMLParagraphElement>('connectionMessage'),
  autoAcceptEnabled: element<HTMLInputElement>('autoAcceptEnabled'),
  readyCheckStatus: element<HTMLParagraphElement>('readyCheckStatus'),
  discordNotificationsEnabled: element<HTMLInputElement>('discordNotificationsEnabled'),
  discordWebhookUrl: element<HTMLInputElement>('discordWebhookUrl'),
  discordUserIds: element<HTMLDivElement>('discordUserIds'),
  addDiscordUserId: element<HTMLButtonElement>('addDiscordUserId'),
  eventQueuePopped: element<HTMLInputElement>('eventQueuePopped'),
  eventAutoAccepted: element<HTMLInputElement>('eventAutoAccepted'),
  eventGameStarted: element<HTMLInputElement>('eventGameStarted'),
  messageQueuePopped: element<HTMLTextAreaElement>('messageQueuePopped'),
  messageAutoAccepted: element<HTMLTextAreaElement>('messageAutoAccepted'),
  messageGameStarted: element<HTMLTextAreaElement>('messageGameStarted'),
  messagePreview: element<HTMLParagraphElement>('messagePreview'),
  testWebhook: element<HTMLButtonElement>('testWebhook'),
  webhookTestStatus: element<HTMLElement>('webhookTestStatus'),
  minimizeToTray: element<HTMLInputElement>('minimizeToTray'),
  saveStatus: element<HTMLSpanElement>('saveStatus')
};

let currentState: AppState | null = null;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
let mentionDrafts: DiscordMention[] = [];

async function save(patch: SettingsPatch): Promise<void> {
  controls.saveStatus.textContent = 'Saving…';
  try {
    applyState(await window.leagueAutoAccept.updateSettings(patch));
    controls.saveStatus.textContent = 'Settings saved locally.';
  } catch {
    controls.saveStatus.textContent = 'Settings could not be saved.';
  }
}

function saveAfterTyping(patch: () => SettingsPatch): void {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    void save(patch());
  }, 300);
}

function setValueUnlessActive(control: HTMLInputElement | HTMLTextAreaElement, value: string): void {
  if (document.activeElement !== control) control.value = value;
}

function applyState(next: AppState): void {
  currentState = next;
  const {settings} = next;
  controls.connectionBadge.className = `status-badge ${next.connectionStatus}`;
  controls.connectionLabel.textContent = next.connectionStatus === 'connected'
    ? 'League connected'
    : next.connectionStatus === 'connecting'
      ? 'League connecting'
      : 'League disconnected';
  controls.connectionMessage.textContent = next.connectionMessage;
  controls.autoAcceptEnabled.checked = settings.autoAcceptEnabled;
  controls.readyCheckStatus.className = `automation-status ${next.readyCheckStatus}`;
  controls.readyCheckStatus.textContent = next.readyCheckMessage;
  controls.discordNotificationsEnabled.checked = settings.discordNotificationsEnabled;
  setValueUnlessActive(controls.discordWebhookUrl, settings.discordWebhookUrl);
  controls.eventQueuePopped.checked = settings.notificationEvents.queuePopped;
  controls.eventAutoAccepted.checked = settings.notificationEvents.autoAccepted;
  controls.eventGameStarted.checked = settings.notificationEvents.gameStarted;
  setValueUnlessActive(controls.messageQueuePopped, settings.notificationMessages.queuePopped);
  setValueUnlessActive(controls.messageAutoAccepted, settings.notificationMessages.autoAccepted);
  setValueUnlessActive(controls.messageGameStarted, settings.notificationMessages.gameStarted);
  controls.minimizeToTray.checked = settings.minimizeToTray;
  if (!controls.discordUserIds.contains(document.activeElement)) {
    mentionDrafts = settings.discordMentions.map(mention => ({...mention}));
    renderMentions(mentionDrafts);
  }
  updatePreview(settings);
}

function renderMentions(mentions: DiscordMention[]): void {
  controls.discordUserIds.replaceChildren();
  mentions.forEach((mention, index) => {
    const row = document.createElement('div');
    row.className = 'user-id-row';
    const nickname = document.createElement('input');
    nickname.type = 'text';
    nickname.autocomplete = 'off';
    nickname.maxLength = MAX_DISCORD_NICKNAME_LENGTH;
    nickname.placeholder = 'Nickname (optional)';
    nickname.value = mention.nickname;
    nickname.dataset['mentionNickname'] = String(index);
    nickname.setAttribute('aria-label', `Nickname for Discord user ${index + 1}`);
    nickname.addEventListener('input', () => {
      mentionDrafts[index] = {...mentionDrafts[index]!, nickname: nickname.value};
    });
    nickname.addEventListener('blur', () => saveMentionsIfValid());

    const userId = document.createElement('input');
    userId.type = 'text';
    userId.inputMode = 'numeric';
    userId.autocomplete = 'off';
    userId.placeholder = 'Discord user ID';
    userId.value = mention.id;
    userId.dataset['mentionId'] = String(index);
    userId.setAttribute('aria-label', `Discord user ID ${index + 1}`);
    userId.addEventListener('input', () => {
      mentionDrafts[index] = {...mentionDrafts[index]!, id: userId.value};
      userId.setCustomValidity(userId.value.trim() && !/^\d{17,20}$/.test(userId.value.trim())
        ? 'Enter a 17–20 digit Discord user ID.'
        : '');
      updatePreviewFromControls();
    });
    userId.addEventListener('blur', () => {
      const value = userId.value.trim();
      if (value && !/^\d{17,20}$/.test(value)) {
        userId.reportValidity();
        return;
      }
      saveMentionsIfValid();
    });
    const remove = document.createElement('button');
    remove.type = 'button';
    remove.className = 'icon-button';
    remove.textContent = 'Remove';
    remove.setAttribute('aria-label', `Remove Discord user ID ${index + 1}`);
    remove.addEventListener('click', () => {
      const next = [...mentionDrafts];
      next.splice(index, 1);
      mentionDrafts = next;
      renderMentions(next);
      updatePreviewFromControls();
      void save({discordMentions: next});
    });
    row.append(nickname, userId, remove);
    controls.discordUserIds.append(row);
  });
  controls.addDiscordUserId.disabled = mentions.length >= MAX_DISCORD_USER_IDS;
}

function mentionsFromInputs(): DiscordMention[] {
  return Array.from(controls.discordUserIds.querySelectorAll<HTMLElement>('.user-id-row')).map(row => ({
    nickname: row.querySelector<HTMLInputElement>('[data-mention-nickname]')?.value.trim() || '',
    id: row.querySelector<HTMLInputElement>('[data-mention-id]')?.value.trim() || ''
  }));
}

function saveMentionsIfValid(): void {
  const mentions = mentionsFromInputs();
  if (mentions.some(mention => !/^\d{17,20}$/.test(mention.id))) return;
  mentionDrafts = mentions;
  void save({discordMentions: mentions});
}

function updatePreview(settings: AppSettings): void {
  const mentions = settings.discordMentions.length
    ? settings.discordMentions.map(mention => mention.nickname
      ? `${mention.nickname} (<@${mention.id}>)`
      : `<@${mention.id}>`).join(' ')
    : '[configured mentions]';
  controls.messagePreview.textContent = settings.notificationMessages.autoAccepted.replaceAll('{mentions}', mentions).trim();
}

function updatePreviewFromControls(): void {
  if (!currentState) return;
  updatePreview({
    ...currentState.settings,
    discordMentions: mentionsFromInputs().filter(mention => /^\d{17,20}$/.test(mention.id)),
    notificationMessages: {
      ...currentState.settings.notificationMessages,
      autoAccepted: controls.messageAutoAccepted.value
    }
  });
}

controls.autoAcceptEnabled.addEventListener('change', () => void save({autoAcceptEnabled: controls.autoAcceptEnabled.checked}));
controls.discordNotificationsEnabled.addEventListener('change', () => {
  void save({discordNotificationsEnabled: controls.discordNotificationsEnabled.checked});
});
controls.discordWebhookUrl.addEventListener('input', () => {
  saveAfterTyping(() => ({discordWebhookUrl: controls.discordWebhookUrl.value}));
});
controls.addDiscordUserId.addEventListener('click', () => {
  mentionDrafts = mentionsFromInputs();
  if (mentionDrafts.length < MAX_DISCORD_USER_IDS) mentionDrafts.push({id: '', nickname: ''});
  renderMentions(mentionDrafts);
  const nicknames = controls.discordUserIds.querySelectorAll<HTMLInputElement>('[data-mention-nickname]');
  nicknames.item(nicknames.length - 1).focus();
});
controls.eventQueuePopped.addEventListener('change', () => {
  void save({notificationEvents: {queuePopped: controls.eventQueuePopped.checked}});
});
controls.eventAutoAccepted.addEventListener('change', () => {
  void save({notificationEvents: {autoAccepted: controls.eventAutoAccepted.checked}});
});
controls.eventGameStarted.addEventListener('change', () => {
  void save({notificationEvents: {gameStarted: controls.eventGameStarted.checked}});
});
controls.messageQueuePopped.addEventListener('input', () => {
  saveAfterTyping(() => ({notificationMessages: {queuePopped: controls.messageQueuePopped.value}}));
});
controls.messageAutoAccepted.addEventListener('input', () => {
  updatePreviewFromControls();
  saveAfterTyping(() => ({notificationMessages: {autoAccepted: controls.messageAutoAccepted.value}}));
});
controls.messageGameStarted.addEventListener('input', () => {
  saveAfterTyping(() => ({notificationMessages: {gameStarted: controls.messageGameStarted.value}}));
});
controls.minimizeToTray.addEventListener('change', () => void save({minimizeToTray: controls.minimizeToTray.checked}));
controls.testWebhook.addEventListener('click', async () => {
  controls.testWebhook.disabled = true;
  controls.webhookTestStatus.textContent = 'Sending test webhook…';
  try {
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    await save({
      discordWebhookUrl: controls.discordWebhookUrl.value,
      discordMentions: mentionsFromInputs()
    });
    const result = await window.leagueAutoAccept.testWebhook();
    controls.webhookTestStatus.textContent = result.message;
  } catch {
    controls.webhookTestStatus.textContent = 'Discord webhook test failed.';
  } finally {
    controls.testWebhook.disabled = false;
  }
});

const unsubscribe = window.leagueAutoAccept.onStateChanged(applyState);
window.addEventListener('beforeunload', unsubscribe, {once: true});

void window.leagueAutoAccept.getState()
  .then(applyState)
  .catch(() => {
    controls.connectionMessage.textContent = 'The secure Electron bridge is unavailable.';
  });
