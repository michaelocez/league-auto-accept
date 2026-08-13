import {app, BrowserWindow, Menu, nativeImage, Tray} from 'electron';
import {join} from 'node:path';
import type {AppState} from '../shared/contracts';

export class TrayController {
  private tray: Tray | null = null;
  private state: AppState | null = null;

  constructor(
    private readonly getWindow: () => BrowserWindow | null,
    private readonly quit: () => void
  ) {
  }

  public sync(state: AppState): void {
    this.state = state;
    if (!state.settings.minimizeToTray) {
      this.destroy();
      return;
    }
    if (!this.tray) this.create();
    this.rebuildMenu();
  }

  public destroy(): void {
    this.tray?.destroy();
    this.tray = null;
  }

  private create(): void {
    const icon = nativeImage.createFromPath(join(app.getAppPath(), 'assets', 'tray-icon.png'));
    if (icon.isEmpty()) throw new Error('The tray icon could not be loaded.');
    this.tray = new Tray(icon);
    this.tray.setToolTip('League Auto Accept');
    this.tray.on('click', () => this.showWindow());
    this.tray.on('double-click', () => this.showWindow());
  }

  private rebuildMenu(): void {
    if (!this.tray || !this.state) return;
    const connectionLabel = this.state.connectionStatus === 'connected'
      ? 'League Client: Connected'
      : 'League Client: Disconnected';
    const autoAcceptLabel = this.state.settings.autoAcceptEnabled
      ? `Auto Accept: On — ${this.state.readyCheckStatus}`
      : 'Auto Accept: Off';
    this.tray.setContextMenu(Menu.buildFromTemplate([
      {label: 'Show League Auto Accept', click: () => this.showWindow()},
      {type: 'separator'},
      {label: connectionLabel, enabled: false},
      {label: autoAcceptLabel, enabled: false},
      {type: 'separator'},
      {label: 'Quit', click: this.quit}
    ]));
  }

  private showWindow(): void {
    const window = this.getWindow();
    if (!window) return;
    if (window.isMinimized()) window.restore();
    window.show();
    window.focus();
  }
}
