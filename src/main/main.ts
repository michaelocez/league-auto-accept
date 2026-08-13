import {app, BrowserWindow, ipcMain, session} from 'electron';
import {join} from 'node:path';
import type {AppState} from '../shared/contracts';
import {IPC_CHANNELS} from '../shared/contracts';
import {AutoAcceptController} from './auto-accept-controller';
import {DiscordNotificationService} from './discord-notification-service';
import {DiscordWebhookClient} from './discord-webhook-client';
import {ElectronSecretCodec} from './electron-secret-codec';
import {ClientLocator} from './lcu/client-locator';
import {LcuController} from './lcu/lcu-controller';
import {SettingsStore} from './settings-store';
import {TrayController} from './tray-controller';

const isDevelopment = process.argv.includes('--serve');
const developmentUrl = 'http://127.0.0.1:5173';
const gotSingleInstanceLock = app.requestSingleInstanceLock();

let mainWindow: BrowserWindow | null = null;
let settingsStore: SettingsStore;
let trayController: TrayController;
let lcuController: LcuController;
let autoAcceptController: AutoAcceptController;
let discordNotifications: DiscordNotificationService;
let isQuitting = false;
let state: AppState;

function contentSecurityPolicy(): string {
  const connectSource = isDevelopment
    ? "connect-src 'self' ws://127.0.0.1:5173"
    : "connect-src 'self'";
  const styleSource = isDevelopment ? "style-src 'self' 'unsafe-inline'" : "style-src 'self'";
  return [
    "default-src 'self'",
    "script-src 'self'",
    styleSource,
    "img-src 'self' data:",
    connectSource,
    "font-src 'self'",
    "object-src 'none'",
    "frame-src 'none'",
    "base-uri 'self'",
    "form-action 'none'"
  ].join('; ');
}

function installSessionSecurity(): void {
  session.defaultSession.setPermissionRequestHandler((_contents, _permission, callback) => callback(false));
  session.defaultSession.setPermissionCheckHandler(() => false);
  session.defaultSession.webRequest.onHeadersReceived((details, callback) => {
    callback({
      responseHeaders: {
        ...details.responseHeaders,
        'Content-Security-Policy': [contentSecurityPolicy()],
        'Referrer-Policy': ['no-referrer'],
        'X-Content-Type-Options': ['nosniff']
      }
    });
  });
}

function createWindow(): BrowserWindow {
  const window = new BrowserWindow({
    title: 'League Auto Accept',
    width: 1120,
    height: 780,
    minWidth: 860,
    minHeight: 640,
    backgroundColor: '#010102',
    icon: join(app.getAppPath(), 'assets', 'icon.png'),
    show: false,
    autoHideMenuBar: true,
    webPreferences: {
      preload: join(__dirname, '..', 'preload', 'index.js'),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      webSecurity: true,
      allowRunningInsecureContent: false,
      devTools: isDevelopment
    }
  });

  window.webContents.setWindowOpenHandler(() => ({action: 'deny'}));
  window.webContents.on('will-attach-webview', event => event.preventDefault());
  window.webContents.on('will-navigate', event => event.preventDefault());
  window.once('ready-to-show', () => window.show());
  window.on('minimize', () => {
    if (!state.settings.minimizeToTray) return;
    window.hide();
  });
  window.on('close', event => {
    if (isQuitting || !state.settings.minimizeToTray) return;
    event.preventDefault();
    window.hide();
  });
  window.on('closed', () => {
    mainWindow = null;
  });

  if (isDevelopment) {
    void window.loadURL(developmentUrl);
  } else {
    void window.loadFile(join(__dirname, '..', '..', 'dist', 'renderer', 'index.html'));
  }
  return window;
}

function assertTrustedSender(sender: Electron.WebContents): void {
  if (!mainWindow || sender.id !== mainWindow.webContents.id) {
    throw new Error('Untrusted IPC sender.');
  }
}

function broadcastState(): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(IPC_CHANNELS.stateChanged, state);
  }
  trayController.sync(state);
}

function registerIpcHandlers(): void {
  ipcMain.handle(IPC_CHANNELS.getState, event => {
    assertTrustedSender(event.sender);
    return state;
  });
  ipcMain.handle(IPC_CHANNELS.updateSettings, async (event, patch: unknown) => {
    assertTrustedSender(event.sender);
    const settings = await settingsStore.update(patch);
    state = {...state, settings};
    autoAcceptController.setEnabled(settings.autoAcceptEnabled);
    broadcastState();
    return state;
  });
  ipcMain.handle(IPC_CHANNELS.testWebhook, event => {
    assertTrustedSender(event.sender);
    return discordNotifications.test();
  });
}

function quitApplication(): void {
  isQuitting = true;
  app.quit();
}

async function bootstrap(): Promise<void> {
  app.setAppUserModelId('nz.co.michaelocez.leagueautoaccept');
  installSessionSecurity();
  settingsStore = new SettingsStore(app.getPath('userData'), new ElectronSecretCodec());
  const settings = await settingsStore.load();
  state = {
    settings,
    connectionStatus: 'connecting',
    connectionMessage: 'Starting League Client connection…',
    readyCheckStatus: 'idle',
    readyCheckMessage: 'Waiting for a ready check.',
    capabilities: {
      lcuConnection: true,
      autoAccept: true,
      discordDelivery: true
    }
  };
  trayController = new TrayController(() => mainWindow, quitApplication);
  discordNotifications = new DiscordNotificationService(
    () => settingsStore.snapshot(),
    new DiscordWebhookClient()
  );
  registerIpcHandlers();
  mainWindow = createWindow();
  trayController.sync(state);
  lcuController = new LcuController(new ClientLocator(), connection => {
    state = {
      ...state,
      connectionStatus: connection.status,
      connectionMessage: connection.message
    };
    autoAcceptController.handleConnectionStatus(connection.status);
    broadcastState();
  });
  autoAcceptController = new AutoAcceptController(
    lcuController,
    settings.autoAcceptEnabled,
    readyCheck => {
      state = {
        ...state,
        readyCheckStatus: readyCheck.status,
        readyCheckMessage: readyCheck.message
      };
      broadcastState();
    },
    {emitAutomationEvent: event => discordNotifications.notify(event)}
  );
  autoAcceptController.start();
  lcuController.start();
}

if (!gotSingleInstanceLock) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (!mainWindow) return;
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    mainWindow.focus();
  });
  app.on('before-quit', () => {
    isQuitting = true;
    lcuController?.stop();
    autoAcceptController?.stop();
    trayController?.destroy();
  });
  app.on('window-all-closed', () => {
    if (!state?.settings.minimizeToTray) app.quit();
  });
  app.on('activate', () => {
    if (!mainWindow) mainWindow = createWindow();
    mainWindow.show();
  });
  void app.whenReady().then(bootstrap);
}
