import type {LeagueAutoAcceptBridge} from '../shared/contracts';

declare global {
  interface Window {
    leagueAutoAccept: LeagueAutoAcceptBridge;
  }
}

export {};
