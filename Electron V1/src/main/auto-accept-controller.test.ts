import assert from 'node:assert/strict';
import {describe, it} from 'node:test';
import {AutoAcceptController, type AutoAcceptState} from './auto-accept-controller';
import type {LcuRestClient} from './lcu/lcu-client';
import type {NotificationEvent} from '../shared/contracts';
import type {LcuJsonApiEvent} from './lcu/types';

const READY_CHECK_ENDPOINT = '/lol-matchmaking/v1/ready-check';
const ACCEPT_ENDPOINT = '/lol-matchmaking/v1/ready-check/accept';

class FakeClient implements LcuRestClient {
  public acceptCalls = 0;
  public failuresRemaining = 0;

  public async verify(): Promise<void> {
  }

  public async requestJson<T>(method: string, endpoint: string): Promise<T> {
    if (method === 'POST' && endpoint === ACCEPT_ENDPOINT) {
      this.acceptCalls++;
      if (this.failuresRemaining-- > 0) throw new Error('simulated failure');
      return null as T;
    }
    throw new Error('snapshot unavailable');
  }
}

class FakeLcu {
  private listener: ((event: LcuJsonApiEvent) => void) | null = null;

  constructor(public client: LcuRestClient | null) {
  }

  public subscribe(listener: (event: LcuJsonApiEvent) => void): () => void {
    this.listener = listener;
    return () => {
      this.listener = null;
    };
  }

  public getClient(): LcuRestClient | null {
    return this.client;
  }

  public ready(playerResponse = 'None'): void {
    this.listener?.({
      uri: READY_CHECK_ENDPOINT,
      eventType: 'Update',
      data: {state: 'InProgress', playerResponse}
    });
  }

  public leaveReadyCheck(): void {
    this.listener?.({uri: READY_CHECK_ENDPOINT, eventType: 'Update', data: {state: 'Completed'}});
  }

  public deleteReadyCheck(): void {
    this.listener?.({uri: READY_CHECK_ENDPOINT, eventType: 'Delete', data: null});
  }

  public phase(value: string): void {
    this.listener?.({uri: '/lol-gameflow/v1/gameflow-phase', eventType: 'Update', data: value});
  }
}

function setup(enabled = true, maxAttempts = 2): {
  client: FakeClient;
  lcu: FakeLcu;
  controller: AutoAcceptController;
  states: AutoAcceptState[];
  events: NotificationEvent[];
} {
  const client = new FakeClient();
  const lcu = new FakeLcu(client);
  const states: AutoAcceptState[] = [];
  const events: NotificationEvent[] = [];
  const controller = new AutoAcceptController(lcu, enabled, state => states.push(state), {
    acceptDelayMs: 1,
    retryDelayMs: 1,
    maxAttempts,
    emitAutomationEvent: event => events.push(event)
  });
  controller.start();
  controller.handleConnectionStatus('connected');
  return {client, lcu, controller, states, events};
}

async function settle(delayMs = 15): Promise<void> {
  await new Promise(resolve => setTimeout(resolve, delayMs));
}

describe('AutoAcceptController', () => {
  it('coalesces duplicate ready-check events into one successful accept', async () => {
    const {client, lcu, states} = setup();
    lcu.ready();
    lcu.ready();
    lcu.ready();
    await settle();
    lcu.ready();
    await settle();

    assert.equal(client.acceptCalls, 1);
    assert.equal(states.at(-1)?.status, 'accepted');
  });

  it('does not accept while disabled and accepts when enabled during the same cycle', async () => {
    const {client, lcu, controller} = setup(false);
    lcu.ready();
    await settle();
    assert.equal(client.acceptCalls, 0);

    controller.setEnabled(true);
    await settle();
    assert.equal(client.acceptCalls, 1);
  });

  it('cancels a scheduled accept when disabled or disconnected', async () => {
    const first = setup();
    first.lcu.ready();
    first.controller.setEnabled(false);
    await settle();
    assert.equal(first.client.acceptCalls, 0);

    const second = setup();
    second.lcu.ready();
    second.controller.handleConnectionStatus('disconnected');
    await settle();
    assert.equal(second.client.acceptCalls, 0);
  });

  it('does not post when the player is already accepted', async () => {
    const {client, lcu, states} = setup();
    lcu.ready('Accepted');
    await settle();

    assert.equal(client.acceptCalls, 0);
    assert.equal(states.at(-1)?.status, 'accepted');
  });

  it('retries a failed request once and stops after the configured bound', async () => {
    const successOnRetry = setup();
    successOnRetry.client.failuresRemaining = 1;
    successOnRetry.lcu.ready();
    await settle(30);
    assert.equal(successOnRetry.client.acceptCalls, 2);
    assert.equal(successOnRetry.states.at(-1)?.status, 'accepted');

    const boundedFailure = setup();
    boundedFailure.client.failuresRemaining = 5;
    boundedFailure.lcu.ready();
    await settle(30);
    assert.equal(boundedFailure.client.acceptCalls, 2);
    assert.equal(boundedFailure.states.at(-1)?.status, 'error');
  });

  it('starts a fresh cycle only after the previous ready check ends', async () => {
    const {client, lcu} = setup();
    lcu.ready();
    await settle();
    lcu.leaveReadyCheck();
    lcu.ready();
    await settle();

    assert.equal(client.acceptCalls, 2);
  });

  it('treats a delete event without data as the end of a ready-check cycle', async () => {
    const {client, lcu} = setup();
    lcu.ready();
    await settle();
    lcu.deleteReadyCheck();
    lcu.ready();
    await settle();

    assert.equal(client.acceptCalls, 2);
  });

  it('emits each lifecycle notification once despite duplicate LCU events', async () => {
    const {lcu, events} = setup();
    lcu.ready();
    lcu.ready();
    await settle();
    lcu.phase('InProgress');
    lcu.phase('InProgress');
    await settle();

    assert.deepEqual(events, ['queuePopped', 'autoAccepted', 'gameStarted']);
  });

  it('keeps a successful accept successful when a notification observer throws', async () => {
    const client = new FakeClient();
    const lcu = new FakeLcu(client);
    const states: AutoAcceptState[] = [];
    const controller = new AutoAcceptController(lcu, true, state => states.push(state), {
      acceptDelayMs: 1,
      retryDelayMs: 1,
      emitAutomationEvent: () => {
        throw new Error('Discord failed');
      }
    });
    controller.start();
    controller.handleConnectionStatus('connected');
    lcu.ready();
    await settle();

    assert.equal(client.acceptCalls, 1);
    assert.equal(states.at(-1)?.status, 'accepted');
  });
});
