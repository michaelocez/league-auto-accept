import {request as httpsRequest} from 'node:https';
import type {LcuCredentials} from './types';

const REQUEST_TIMEOUT_MS = 5000;
const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;
const MAX_REQUEST_BYTES = 1024 * 1024;
const ALLOWED_METHODS = new Set(['GET', 'POST', 'PUT', 'PATCH', 'DELETE']);

export interface LcuRestClient {
  verify(): Promise<void>;
  requestJson<T>(method: string, endpoint: string, body?: unknown): Promise<T>;
}

export class LcuClient implements LcuRestClient {
  private readonly authorization: string;

  constructor(private readonly credentials: LcuCredentials) {
    this.authorization = `Basic ${Buffer.from(`riot:${credentials.password}`, 'utf8').toString('base64')}`;
  }

  public async verify(): Promise<void> {
    await this.requestJson('GET', '/lol-summoner/v1/current-summoner');
  }

  public requestJson<T>(methodValue: string, endpoint: string, body?: unknown): Promise<T> {
    const method = String(methodValue || '').trim().toUpperCase();
    if (!ALLOWED_METHODS.has(method)) return Promise.reject(new Error('Unsupported LCU request method.'));
    if (!isAllowedLcuEndpoint(endpoint)) return Promise.reject(new Error('Unsupported LCU endpoint.'));

    let requestBody = '';
    if (body !== undefined) {
      requestBody = JSON.stringify(body);
      if (Buffer.byteLength(requestBody, 'utf8') > MAX_REQUEST_BYTES) {
        return Promise.reject(new Error('LCU request body exceeded the supported size limit.'));
      }
    }

    return new Promise<T>((resolve, reject) => {
      let settled = false;
      const finishError = (error: Error): void => {
        if (settled) return;
        settled = true;
        reject(error);
      };
      const headers: Record<string, string | number> = {
        Accept: 'application/json',
        Authorization: this.authorization
      };
      if (requestBody) {
        headers['Content-Type'] = 'application/json';
        headers['Content-Length'] = Buffer.byteLength(requestBody, 'utf8');
      }

      const request = httpsRequest({
        protocol: 'https:',
        hostname: '127.0.0.1',
        port: this.credentials.port,
        path: endpoint,
        method,
        headers,
        rejectUnauthorized: false
      }, response => {
        let responseText = '';
        let responseBytes = 0;
        response.setEncoding('utf8');
        response.on('data', chunk => {
          responseBytes += Buffer.byteLength(chunk, 'utf8');
          if (responseBytes > MAX_RESPONSE_BYTES) {
            const error = new Error('LCU response exceeded the supported size limit.');
            response.destroy(error);
            finishError(error);
            return;
          }
          responseText += chunk;
        });
        response.once('error', finishError);
        response.on('end', () => {
          if (settled) return;
          const status = response.statusCode || 0;
          if (status < 200 || status >= 300) {
            finishError(new Error(`LCU returned HTTP ${status || 'error'}.`));
            return;
          }
          try {
            const value = responseText ? JSON.parse(responseText) : null;
            settled = true;
            resolve(value as T);
          } catch {
            finishError(new Error('LCU returned an invalid JSON response.'));
          }
        });
      });
      request.setTimeout(REQUEST_TIMEOUT_MS, () => request.destroy(new Error('LCU request timed out.')));
      request.once('error', finishError);
      if (requestBody) request.write(requestBody);
      request.end();
    });
  }
}

export function isAllowedLcuEndpoint(endpoint: string): boolean {
  const value = String(endpoint || '');
  if (!value.startsWith('/lol-') || value.length > 4096) return false;
  if (containsControlCharacters(value) || /[\s\\#]/.test(value) || /%(?:2e|2f|5c)/i.test(value)) return false;
  if (/^[a-z][a-z0-9+.-]*:/i.test(value) || value.startsWith('//')) return false;
  return true;
}

function containsControlCharacters(value: string): boolean {
  return Array.from(value).some(character => {
    const code = character.charCodeAt(0);
    return code <= 31 || code === 127;
  });
}
