import assert from 'node:assert/strict';
import {describe, it} from 'node:test';
import {isAllowedLcuEndpoint} from './lcu-client';

describe('LCU endpoint validation', () => {
  it('allows League Client API paths', () => {
    assert.equal(isAllowedLcuEndpoint('/lol-summoner/v1/current-summoner'), true);
    assert.equal(isAllowedLcuEndpoint('/lol-gameflow/v1/gameflow-phase'), true);
  });

  it('rejects external, protocol-relative, and malformed paths', () => {
    assert.equal(isAllowedLcuEndpoint('https://example.test/lol-summoner/v1/current-summoner'), false);
    assert.equal(isAllowedLcuEndpoint('//example.test/lol-summoner/v1/current-summoner'), false);
    assert.equal(isAllowedLcuEndpoint('/lol-test/%2e%2e/secret'), false);
    assert.equal(isAllowedLcuEndpoint('/riotclient/command-line-args'), false);
  });
});
