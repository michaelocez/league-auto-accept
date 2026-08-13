import type {ConnectionStatus, NotificationEvent, ReadyCheckStatus} from '../shared/contracts';
import type {LcuController} from './lcu/lcu-controller';
import type {LcuJsonApiEvent} from './lcu/types';

const READY_CHECK_ENDPOINT = '/lol-matchmaking/v1/ready-check';
const ACCEPT_ENDPOINT = '/lol-matchmaking/v1/ready-check/accept';
const GAMEFLOW_ENDPOINT = '/lol-gameflow/v1/gameflow-phase';

interface ReadyCheckData {
  state?: unknown;
  playerResponse?: unknown;
}

export interface AutoAcceptState {
  status: ReadyCheckStatus;
  message: string;
}

export interface AutoAcceptControllerOptions {
  acceptDelayMs?: number;
  retryDelayMs?: number;
  maxAttempts?: number;
  emitAutomationEvent?: (event: NotificationEvent) => void;
}

export class AutoAcceptController {
  private readonly acceptDelayMs: number;
  private readonly retryDelayMs: number;
  private readonly maxAttempts: number;
  private unsubscribe: (() => void) | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private enabled: boolean;
  private connected = false;
  private readyCheckActive = false;
  private accepted = false;
  private requestInFlight = false;
  private attempts = 0;
  private generation = 0;
  private lastStateKey = '';
  private lastGameflowPhase = '';
  private readonly emitAutomationEvent: (event: NotificationEvent) => void;

  constructor(
    private readonly lcu: Pick<LcuController, 'subscribe' | 'getClient'>,
    enabled: boolean,
    private readonly emitState: (state: AutoAcceptState) => void,
    options: AutoAcceptControllerOptions = {}
  ) {
    this.enabled = enabled;
    this.acceptDelayMs = options.acceptDelayMs ?? 50;
    this.retryDelayMs = options.retryDelayMs ?? 250;
    this.maxAttempts = options.maxAttempts ?? 2;
    this.emitAutomationEvent = options.emitAutomationEvent ?? (() => undefined);
  }

  public start(): void {
    if (this.unsubscribe) return;
    this.unsubscribe = this.lcu.subscribe(event => this.handleEvent(event));
    this.publishIdle();
  }

  public stop(): void {
    this.unsubscribe?.();
    this.unsubscribe = null;
    this.resetCycle();
    this.connected = false;
  }

  public setEnabled(enabled: boolean): void {
    if (this.enabled === enabled) return;
    this.enabled = enabled;
    if (!enabled) {
      this.cancelTimer();
      if (this.readyCheckActive && !this.accepted && !this.requestInFlight) {
        this.publish({status: 'ready', message: 'Ready check detected; Auto Accept is off.'});
      }
      return;
    }
    if (this.connected && this.readyCheckActive && !this.accepted) this.scheduleAccept(this.acceptDelayMs);
  }

  public handleConnectionStatus(status: ConnectionStatus): void {
    const connected = status === 'connected';
    if (this.connected === connected) return;
    this.connected = connected;
    this.generation++;
    this.cancelTimer();
    if (!connected) {
      this.resetCycle();
      this.lastGameflowPhase = '';
      this.publishIdle();
      return;
    }
    void this.bootstrap(this.generation);
  }

  private async bootstrap(generation: number): Promise<void> {
    const client = this.lcu.getClient();
    if (!client) return;
    try {
      const phase = await client.requestJson<unknown>('GET', GAMEFLOW_ENDPOINT);
      if (generation === this.generation) this.handleGameflowPhase(phase, false);
    } catch {
      // Live events remain authoritative when an optional snapshot is unavailable.
    }
    try {
      const readyCheck = await client.requestJson<unknown>('GET', READY_CHECK_ENDPOINT);
      if (generation === this.generation) this.handleReadyCheck(readyCheck);
    } catch {
      // No ready check commonly returns an error; wait for the next event.
    }
  }

  private handleEvent(event: LcuJsonApiEvent): void {
    if (event.uri === READY_CHECK_ENDPOINT) {
      this.handleReadyCheck(event.eventType.toLowerCase() === 'delete' ? {state: 'Completed'} : event.data);
    } else if (event.uri === GAMEFLOW_ENDPOINT) {
      this.handleGameflowPhase(event.data);
    }
  }

  private handleGameflowPhase(value: unknown, emitEvent = true): void {
    if (typeof value !== 'string') return;
    const phase = value.toLowerCase();
    if (phase === 'inprogress' && this.lastGameflowPhase !== 'inprogress' && emitEvent) {
      this.emitEvent('gameStarted');
    }
    this.lastGameflowPhase = phase;
    if (!['readycheck', 'matchmaking'].includes(phase) && this.readyCheckActive) {
      this.resetCycle();
    }
    if (phase === 'matchmaking' && !this.readyCheckActive) {
      this.publish({status: 'searching', message: 'Searching for a match.'});
    } else if (!['readycheck', 'matchmaking'].includes(phase) && !this.readyCheckActive) {
      this.publishIdle();
    }
  }

  private handleReadyCheck(value: unknown): void {
    if (!isRecord(value)) return;
    const data = value as ReadyCheckData;
    const state = typeof data.state === 'string' ? data.state.toLowerCase() : '';
    if (state !== 'inprogress') {
      if (this.readyCheckActive) {
        this.resetCycle();
        this.publishIdle();
      }
      return;
    }

    if (!this.readyCheckActive) {
      this.generation++;
      this.readyCheckActive = true;
      this.accepted = false;
      this.requestInFlight = false;
      this.attempts = 0;
      this.emitEvent('queuePopped');
    }
    const response = typeof data.playerResponse === 'string' ? data.playerResponse.toLowerCase() : '';
    if (response === 'accepted') {
      this.accepted = true;
      this.cancelTimer();
      this.publish({status: 'accepted', message: 'Ready check accepted.'});
      return;
    }
    if (this.accepted || this.requestInFlight || this.timer) return;
    this.publish({
      status: 'ready',
      message: this.enabled ? 'Ready check detected; accepting…' : 'Ready check detected; Auto Accept is off.'
    });
    if (this.enabled && this.connected) this.scheduleAccept(this.acceptDelayMs);
  }

  private scheduleAccept(delayMs: number): void {
    if (this.timer || this.requestInFlight || this.accepted || !this.readyCheckActive) return;
    const generation = this.generation;
    this.timer = setTimeout(() => {
      this.timer = null;
      if (generation !== this.generation) return;
      void this.accept(generation);
    }, delayMs);
  }

  private async accept(generation: number): Promise<void> {
    if (!this.enabled || !this.connected || !this.readyCheckActive || this.accepted || this.requestInFlight) return;
    const client = this.lcu.getClient();
    if (!client) return;
    this.requestInFlight = true;
    this.attempts++;
    let shouldRetry = false;
    this.publish({status: 'accepting', message: 'Accepting ready check…'});
    try {
      await client.requestJson('POST', ACCEPT_ENDPOINT);
      if (generation !== this.generation) return;
      this.accepted = true;
      this.publish({status: 'accepted', message: 'Ready check auto accepted.'});
      this.emitEvent('autoAccepted');
    } catch {
      if (generation !== this.generation) return;
      if (this.enabled && this.connected && this.readyCheckActive && this.attempts < this.maxAttempts) {
        this.publish({status: 'ready', message: 'Accept attempt failed; retrying once…'});
        shouldRetry = true;
      } else {
        this.publish({status: 'error', message: 'Could not auto accept this ready check.'});
      }
    } finally {
      if (generation === this.generation) {
        this.requestInFlight = false;
        if (shouldRetry) this.scheduleAccept(this.retryDelayMs);
      }
    }
  }

  private resetCycle(): void {
    this.generation++;
    this.cancelTimer();
    this.readyCheckActive = false;
    this.accepted = false;
    this.requestInFlight = false;
    this.attempts = 0;
  }

  private cancelTimer(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  private publishIdle(): void {
    this.publish({status: 'idle', message: 'Waiting for a ready check.'});
  }

  private emitEvent(event: NotificationEvent): void {
    try {
      this.emitAutomationEvent(event);
    } catch {
      // Notification observers must never affect ready-check automation.
    }
  }

  private publish(state: AutoAcceptState): void {
    const key = `${state.status}:${state.message}`;
    if (key === this.lastStateKey) return;
    this.lastStateKey = key;
    this.emitState(state);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
