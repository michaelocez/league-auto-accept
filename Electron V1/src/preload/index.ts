import {contextBridge, ipcRenderer} from 'electron';
import type {AppState, LeagueAutoAcceptBridge, SettingsPatch} from '../shared/contracts';

const channels = {
  getState: 'laa:state:get',
  updateSettings: 'laa:settings:update',
  testWebhook: 'laa:discord:test',
  stateChanged: 'laa:state:changed'
} as const;

const bridge: LeagueAutoAcceptBridge = {
  getState: (): Promise<AppState> => ipcRenderer.invoke(channels.getState),
  updateSettings: (patch: SettingsPatch): Promise<AppState> => ipcRenderer.invoke(channels.updateSettings, patch),
  testWebhook: () => ipcRenderer.invoke(channels.testWebhook),
  onStateChanged: (callback: (state: AppState) => void): (() => void) => {
    const listener = (_event: Electron.IpcRendererEvent, state: AppState): void => callback(state);
    ipcRenderer.on(channels.stateChanged, listener);
    return () => ipcRenderer.removeListener(channels.stateChanged, listener);
  }
};

contextBridge.exposeInMainWorld('leagueAutoAccept', bridge);
