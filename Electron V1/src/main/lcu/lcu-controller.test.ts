import assert from 'node:assert/strict';
import {describe, it} from 'node:test';
import type {LcuClientLocator} from './client-locator';
import {LcuController, type LcuControllerState} from './lcu-controller';
import type {LcuRestClient} from './lcu-client';
import type {LcuEventConnection} from './lcu-event-socket';
import type {LcuCredentials, LcuEventConnectionState, LcuJsonApiEvent} from './types';

const credentials: LcuCredentials = {
  installPath: 'C:\\Riot Games\\League of Legends',
  processName: 'LeagueClientUx',
  processId: 1234,
  port: 54321,
  password: 'local-secret',
  protocol: 'https'
};

class FakeLocator implements LcuClientLocator {
  public result: LcuCredentials | null = credentials;

  public async findRunningClient(): Promise<LcuCredentials | null> {
    return this.result;
  }
}

class FakeClient implements LcuRestClient {
  public verifyCalls = 0;

  public async verify(): Promise<void> {
    this.verifyCalls++;
  }

  public async requestJson<T>(): Promise<T> {
    return null as T;
  }
}

class FakeEventConnection implements LcuEventConnection {
  public closed = false;

  constructor(
    private readonly emitEvent: (event: LcuJsonApiEvent) => void,
    private readonly emitState: (state: LcuEventConnectionState) => void
  ) {
  }

  public connect(): void {
    this.emitState({connected: false, connecting: true, message: 'Connecting'});
    this.emitState({connected: true, connecting: false, message: 'League Client connected.'});
  }

  public close(): void {
    this.closed = true;
  }

  public disconnect(): void {
    this.emitState({connected: false, connecting: false, message: 'Disconnected.'});
  }

  public event(event: LcuJsonApiEvent): void {
    this.emitEvent(event);
  }
}

describe('LcuController', () => {
  it('verifies REST, connects events, reconnects events, and clears state when League closes', async () => {
    const locator = new FakeLocator();
    const client = new FakeClient();
    const states: LcuControllerState[] = [];
    const connections: FakeEventConnection[] = [];
    const events: LcuJsonApiEvent[] = [];
    const controller = new LcuController(locator, state => states.push(state), {
      clientFactory: () => client,
      eventFactory: (_credentials, emitEvent, emitState) => {
        const connection = new FakeEventConnection(emitEvent, emitState);
        connections.push(connection);
        return connection;
      }
    });
    controller.subscribe(event => events.push(event));

    await controller.checkNow();
    assert.equal(states.at(-1)?.status, 'connected');
    assert.equal(client.verifyCalls, 1);
    assert.equal(connections.length, 1);
    assert.equal(controller.getClient(), client);

    connections[0]?.event({uri: '/lol-gameflow/v1/gameflow-phase', eventType: 'Update', data: 'Lobby'});
    assert.equal(events.length, 1);

    connections[0]?.disconnect();
    await controller.checkNow();
    assert.equal(connections.length, 2);
    assert.equal(client.verifyCalls, 1);
    assert.equal(states.at(-1)?.status, 'connected');

    locator.result = null;
    await controller.checkNow();
    assert.equal(connections[1]?.closed, true);
    assert.equal(controller.getClient(), null);
    assert.equal(states.at(-1)?.status, 'disconnected');
  });
});
