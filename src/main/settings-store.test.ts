import assert from 'node:assert/strict';
import {mkdtemp, readFile, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {afterEach, describe, it} from 'node:test';
import {SettingsStore, type SecretCodec, type StoredSecret} from './settings-store';

const directories: string[] = [];
const codec: SecretCodec = {
  encode: value => ({protection: 'os', value: Buffer.from(`protected:${value}`).toString('base64')}),
  decode: (secret: StoredSecret) => Buffer.from(secret.value, 'base64').toString('utf8').replace(/^protected:/, '')
};

async function directory(): Promise<string> {
  const value = await mkdtemp(join(tmpdir(), 'league-auto-accept-'));
  directories.push(value);
  return value;
}

afterEach(async () => {
  await Promise.all(directories.splice(0).map(value => rm(value, {recursive: true, force: true})));
});

describe('SettingsStore', () => {
  it('creates defaults and persists updates', async () => {
    const root = await directory();
    const store = new SettingsStore(root, codec);
    const defaults = await store.load();
    assert.equal(defaults.minimizeToTray, true);

    await store.update({autoAcceptEnabled: true, discordWebhookUrl: 'https://discord.com/api/webhooks/example'});
    const reloaded = await new SettingsStore(root, codec).load();
    assert.equal(reloaded.autoAcceptEnabled, true);
    assert.equal(reloaded.discordWebhookUrl, 'https://discord.com/api/webhooks/example');

    const stored = await readFile(join(root, 'settings.json'), 'utf8');
    assert.equal(stored.includes('https://discord.com'), false);
    assert.equal(stored.includes('"protection": "os"'), true);
  });

  it('recovers from a corrupt settings file', async () => {
    const root = await directory();
    await writeFile(join(root, 'settings.json'), '{not-json', 'utf8');
    const settings = await new SettingsStore(root, codec).load();
    assert.equal(settings.autoAcceptEnabled, false);
    assert.equal(settings.notificationEvents.autoAccepted, true);
  });

  it('migrates and persists legacy Discord user IDs without losing them', async () => {
    const root = await directory();
    await writeFile(join(root, 'settings.json'), JSON.stringify({
      schemaVersion: 1,
      discordUserIds: ['12345678901234567'],
      discordWebhookUrl: {protection: 'plain', value: ''}
    }), 'utf8');
    const store = new SettingsStore(root, codec);
    const settings = await store.load();
    assert.deepEqual(settings.discordMentions, [{id: '12345678901234567', nickname: ''}]);

    await store.update({discordMentions: [{id: '12345678901234567', nickname: 'Michael'}]});
    const reloaded = await new SettingsStore(root, codec).load();
    assert.deepEqual(reloaded.discordMentions, [{id: '12345678901234567', nickname: 'Michael'}]);
  });
});
