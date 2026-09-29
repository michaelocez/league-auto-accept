import {request as httpsRequest} from 'node:https';
import type {WebhookResult} from '../shared/contracts';

const ALLOWED_HOSTS = new Set([
  'discord.com',
  'canary.discord.com',
  'ptb.discord.com',
  'discordapp.com'
]);
const WEBHOOK_PATH = /^\/api(?:\/v\d+)?\/webhooks\/\d{17,20}\/[A-Za-z0-9._-]{20,200}\/?$/;
const REQUEST_TIMEOUT_MS = 7000;
const MAX_RESPONSE_BYTES = 8 * 1024;
const MAX_CONTENT_LENGTH = 2000;

export interface DiscordWebhookSender {
  send(webhookUrl: string, content: string, userIds: string[]): Promise<WebhookResult>;
}

export class DiscordWebhookClient implements DiscordWebhookSender {
  public send(webhookUrl: string, content: string, userIds: string[]): Promise<WebhookResult> {
    const target = parseDiscordWebhookUrl(webhookUrl);
    if (!target) return Promise.resolve({ok: false, message: 'Enter a valid Discord webhook URL.'});
    if (!content.trim() || content.length > MAX_CONTENT_LENGTH) {
      return Promise.resolve({ok: false, message: 'The Discord message must contain 1–2,000 characters.'});
    }
    if (!userIds.every(id => /^\d{17,20}$/.test(id))) {
      return Promise.resolve({ok: false, message: 'One or more Discord user IDs are invalid.'});
    }

    const body = discordWebhookPayload(content, userIds);
    return new Promise(resolve => {
      let settled = false;
      const finish = (result: WebhookResult): void => {
        if (settled) return;
        settled = true;
        resolve(result);
      };
      const request = httpsRequest({
        protocol: 'https:',
        hostname: target.hostname,
        path: target.pathname,
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(body, 'utf8'),
          'User-Agent': 'League-Auto-Accept'
        }
      }, response => {
        let responseBytes = 0;
        response.on('data', chunk => {
          responseBytes += Buffer.byteLength(chunk);
          if (responseBytes > MAX_RESPONSE_BYTES) {
            response.destroy();
            finish({ok: false, message: 'Discord webhook response was too large.'});
          }
        });
        response.on('error', () => finish({ok: false, message: 'Discord webhook request failed.'}));
        response.on('end', () => {
          const status = response.statusCode || 0;
          finish(status >= 200 && status < 300
            ? {ok: true, message: 'Discord webhook sent.'}
            : {ok: false, message: `Discord returned HTTP ${status || 'error'}.`});
        });
      });
      request.setTimeout(REQUEST_TIMEOUT_MS, () => {
        request.destroy();
        finish({ok: false, message: 'Discord webhook request timed out.'});
      });
      request.on('error', () => finish({ok: false, message: 'Discord webhook request failed.'}));
      request.end(body);
    });
  }
}

export function discordWebhookPayload(content: string, userIds: string[]): string {
  return JSON.stringify({
    content,
    allowed_mentions: {parse: [], users: userIds}
  });
}

export function parseDiscordWebhookUrl(value: unknown): URL | null {
  if (typeof value !== 'string' || value.length > 2048) return null;
  try {
    const target = new URL(value.trim());
    if (target.protocol !== 'https:' || target.port || target.username || target.password) return null;
    if (!ALLOWED_HOSTS.has(target.hostname.toLowerCase())) return null;
    if (!WEBHOOK_PATH.test(target.pathname) || target.search || target.hash) return null;
    return target;
  } catch {
    return null;
  }
}
