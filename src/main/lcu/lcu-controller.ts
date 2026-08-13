import type {ConnectionStatus} from '../../shared/contracts';
import type {LcuClientLocator} from './client-locator';
import {LcuClient, type LcuRestClient} from './lcu-client';
import {LcuEventSocket, type LcuEventConnection} from './lcu-event-socket';
import {
  lcuCredentialSignature,
  type LcuCredentials,
  type LcuEventConnectionState,
  type LcuJsonApiEvent
} from './types';

export interface LcuControllerState {
  status: ConnectionStatus;
  message: string;
}

export interface LcuControllerOptions {
  pollIntervalMs?: number;
  clientFactory?: (credentials: LcuCredentials) => LcuRestClient;
  eventFactory?: (
    credentials: LcuCredentials,
    emitEvent: (event: LcuJsonApiEvent) => void,
    emitState: (state: LcuEventConnectionState) => void
  ) => LcuEventConnection;
}

export class LcuController {
  private readonly pollIntervalMs: number;
  private readonly clientFactory: (credentials: LcuCredentials) => LcuRestClient;
  private readonly eventFactory: LcuControllerOptions['eventFactory'];
  private readonly listeners = new Set<(event: LcuJsonApiEvent) => void>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private checking = false;
  private generation = 0;
  private currentSignature = '';
  private credentials: LcuCredentials | null = null;
  private client: LcuRestClient | null = null;
  private eventConnection: LcuEventConnection | null = null;
  private verified = false;
  private lastStateKey = '';

  constructor(
    private readonly locator: LcuClientLocator,
    private readonly emitState: (state: LcuControllerState) => void,
    options: LcuControllerOptions = {}
  ) {
    this.pollIntervalMs = options.pollIntervalMs ?? 3000;
    this.clientFactory = options.clientFactory ?? (credentials => new LcuClient(credentials));
    this.eventFactory = options.eventFactory ?? ((credentials, emitEvent, emitConnectionState) => {
      return new LcuEventSocket(credentials, emitEvent, emitConnectionState);
    });
  }

  public start(): void {
    if (this.timer) return;
    this.publish({status: 'connecting', message: 'Looking for a running League Client…'});
    void this.checkNow();
    this.timer = setInterval(() => void this.checkNow(), this.pollIntervalMs);
  }

  public stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    this.generation++;
    this.clearConnection();
  }

  public subscribe(listener: (event: LcuJsonApiEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  public getClient(): LcuRestClient | null {
    return this.verified ? this.client : null;
  }

  public async checkNow(): Promise<void> {
    if (this.checking) return;
    this.checking = true;
    try {
      const nextCredentials = await this.locator.findRunningClient();
      if (!nextCredentials) {
        if (this.currentSignature || this.verified || this.eventConnection) this.clearConnection();
        this.publish({status: 'disconnected', message: 'Open the League Client to connect.'});
        return;
      }

      const signature = lcuCredentialSignature(nextCredentials);
      if (signature !== this.currentSignature) {
        this.generation++;
        this.clearConnection();
        this.currentSignature = signature;
        this.credentials = nextCredentials;
      }
      if (!this.verified) {
        await this.verifyConnection();
      } else if (!this.eventConnection) {
        this.startEventConnection();
      }
    } finally {
      this.checking = false;
    }
  }

  private async verifyConnection(): Promise<void> {
    if (!this.credentials) return;
    const generation = this.generation;
    this.publish({status: 'connecting', message: 'Authenticating with the League Client…'});
    const client = this.clientFactory(this.credentials);
    try {
      await client.verify();
      if (generation !== this.generation) return;
      this.client = client;
      this.verified = true;
      this.startEventConnection();
    } catch {
      if (generation !== this.generation) return;
      this.client = null;
      this.verified = false;
      this.currentSignature = '';
      this.credentials = null;
      this.publish({status: 'disconnected', message: 'League Client was found but authentication failed; retrying…'});
    }
  }

  private startEventConnection(): void {
    if (!this.credentials || this.eventConnection) return;
    let connection: LcuEventConnection;
    connection = this.eventFactory!(
      this.credentials,
      event => {
        for (const listener of this.listeners) listener(event);
      },
      connectionState => this.handleEventState(connection, connectionState)
    );
    this.eventConnection = connection;
    connection.connect();
  }

  private handleEventState(connection: LcuEventConnection, connectionState: LcuEventConnectionState): void {
    if (this.eventConnection !== connection) return;
    if (connectionState.connecting) {
      this.publish({status: 'connecting', message: connectionState.message});
      return;
    }
    if (connectionState.connected) {
      this.publish({status: 'connected', message: connectionState.message});
      return;
    }
    this.eventConnection = null;
    this.publish({status: 'connecting', message: `${connectionState.message} Reconnecting…`});
  }

  private clearConnection(): void {
    this.eventConnection?.close();
    this.eventConnection = null;
    this.client = null;
    this.verified = false;
    this.currentSignature = '';
    this.credentials = null;
  }

  private publish(state: LcuControllerState): void {
    const key = `${state.status}:${state.message}`;
    if (key === this.lastStateKey) return;
    this.lastStateKey = key;
    this.emitState(state);
  }
}
