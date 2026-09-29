import assert from 'node:assert/strict';
import {describe, it} from 'node:test';
import {discordWebhookPayload, parseDiscordWebhookUrl} from './discord-webhook-client';

const validPath = `/api/webhooks/${'1'.repeat(17)}/${'token'.repeat(8)}`;

describe('Discord webhook URL validation', () => {
  it('allows only known Discord HTTPS webhook URLs', () => {
    assert.equal(parseDiscordWebhookUrl(`https://discord.com${validPath}`)?.hostname, 'discord.com');
    assert.equal(parseDiscordWebhookUrl(`https://canary.discord.com${validPath}`)?.hostname, 'canary.discord.com');
    assert.equal(parseDiscordWebhookUrl(`https://discord.com/api/v10${validPath.slice(4)}`)?.hostname, 'discord.com');
  });

  it('rejects lookalike hosts, credentials, ports, queries, and malformed paths', () => {
    const invalid = [
      `http://discord.com${validPath}`,
      `https://discord.com.evil.test${validPath}`,
      `https://user:pass@discord.com${validPath}`,
      `https://discord.com:444${validPath}`,
      `https://discord.com${validPath}?wait=true`,
      `https://discord.com/api/webhooks/not-an-id/token`
    ];
    for (const value of invalid) assert.equal(parseDiscordWebhookUrl(value), null);
  });

  it('allows mentions only for explicitly configured user IDs', () => {
    const payload = JSON.parse(discordWebhookPayload('@everyone <@12345678901234567>', [
      '12345678901234567'
    ])) as {allowed_mentions: {parse: string[]; users: string[]}};
    assert.deepEqual(payload.allowed_mentions, {parse: [], users: ['12345678901234567']});
  });
});
