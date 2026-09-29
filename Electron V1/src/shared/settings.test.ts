import assert from 'node:assert/strict';
import {describe, it} from 'node:test';
import {DEFAULT_SETTINGS, MAX_DISCORD_USER_IDS, mergeSettings, normalizeSettings} from './settings';

describe('settings normalization', () => {
  it('uses safe defaults for malformed settings', () => {
    const settings = normalizeSettings({
      autoAcceptEnabled: 'yes',
      discordUserIds: ['bad', '12345678901234567', '12345678901234567']
    });
    assert.equal(settings.autoAcceptEnabled, false);
    assert.deepEqual(settings.discordMentions, [{id: '12345678901234567', nickname: ''}]);
    assert.equal(settings.schemaVersion, 2);
    assert.equal(settings.notificationEvents.autoAccepted, true);
  });

  it('deduplicates and caps Discord user IDs', () => {
    const ids = Array.from({length: 8}, (_, index) => `${index + 1}`.repeat(17).slice(0, 17));
    const settings = normalizeSettings({discordMentions: [
      ...ids.map((id, index) => ({id, nickname: ` Player ${index + 1} `})),
      {id: ids[0], nickname: 'Duplicate'}
    ]});
    assert.equal(settings.discordMentions.length, MAX_DISCORD_USER_IDS);
    assert.equal(new Set(settings.discordMentions.map(mention => mention.id)).size, MAX_DISCORD_USER_IDS);
    assert.equal(settings.discordMentions[0]?.nickname, 'Player 1');
  });

  it('migrates legacy user ID arrays into nickname-ready records', () => {
    const settings = normalizeSettings({schemaVersion: 1, discordUserIds: ['12345678901234567']});
    assert.equal(settings.schemaVersion, 2);
    assert.deepEqual(settings.discordMentions, [{id: '12345678901234567', nickname: ''}]);
  });

  it('merges nested patches without discarding other event settings', () => {
    const settings = mergeSettings(DEFAULT_SETTINGS, {
      notificationEvents: {queuePopped: true}
    });
    assert.equal(settings.notificationEvents.queuePopped, true);
    assert.equal(settings.notificationEvents.autoAccepted, true);
    assert.equal(settings.notificationEvents.gameStarted, false);
  });
});
