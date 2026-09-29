import {createHash, randomBytes} from 'node:crypto';
import {connect as tlsConnect, type TLSSocket} from 'node:tls';
import type {LcuCredentials, LcuEventConnectionState, LcuJsonApiEvent} from './types';
import {WebSocketFrameDecoder} from './websocket-frame-decoder';

const HANDSHAKE_TIMEOUT_MS = 10000;
const MAX_HANDSHAKE_BYTES = 16 * 1024;
const MAX_EVENT_BYTES = 4 * 1024 * 1024;
const WEBSOCKET_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

export interface LcuEventConnection {
  connect(): void;
  close(): void;
}

export class LcuEventSocket implements LcuEventConnection {
  private socket: TLSSocket | null = null;
  private handshakeBuffer = Buffer.alloc(0);
  private handshakeComplete = false;
  private closed = false;
  private key = '';
  private readonly decoder = new WebSocketFrameDecoder(MAX_EVENT_BYTES);

  constructor(
    private readonly credentials: LcuCredentials,
    private readonly emitEvent: (event: LcuJsonApiEvent) => void,
    private readonly emitState: (state: LcuEventConnectionState) => void
  ) {
  }

  public connect(): void {
    this.emitState({connected: false, connecting: true, message: 'Connecting to League Client events…'});
    this.key = randomBytes(16).toString('base64');
    this.socket = tlsConnect({
      host: '127.0.0.1',
      port: this.credentials.port,
      rejectUnauthorized: false
    });
    this.socket.setNoDelay(true);
    this.socket.setTimeout(HANDSHAKE_TIMEOUT_MS, () => {
      if (!this.handshakeComplete) this.finish('League Client event handshake timed out.');
    });
    this.socket.once('secureConnect', () => this.sendHandshake());
    this.socket.on('data', chunk => this.receive(typeof chunk === 'string' ? Buffer.from(chunk, 'utf8') : chunk));
    this.socket.once('error', () => this.finish('League Client event connection failed.'));
    this.socket.once('end', () => this.finish('League Client event connection ended.'));
    this.socket.once('close', () => this.finish('League Client event connection closed.'));
  }

  public close(): void {
    if (this.closed) return;
    this.closed = true;
    this.socket?.removeAllListeners();
    if (this.socket && !this.socket.destroyed) this.socket.destroy();
    this.socket = null;
  }

  private sendHandshake(): void {
    const authorization = Buffer.from(`riot:${this.credentials.password}`, 'utf8').toString('base64');
    const headers = [
      'GET / HTTP/1.1',
      `Host: 127.0.0.1:${this.credentials.port}`,
      'Upgrade: websocket',
      'Connection: Upgrade',
      `Sec-WebSocket-Key: ${this.key}`,
      'Sec-WebSocket-Version: 13',
      `Authorization: Basic ${authorization}`
    ];
    this.socket?.write(`${headers.join('\r\n')}\r\n\r\n`);
  }

  private receive(chunk: Buffer): void {
    if (!this.handshakeComplete) {
      this.handshakeBuffer = Buffer.concat([this.handshakeBuffer, chunk]);
      if (this.handshakeBuffer.length > MAX_HANDSHAKE_BYTES) {
        this.finish('League Client event handshake exceeded its size limit.');
        return;
      }
      const marker = this.handshakeBuffer.indexOf('\r\n\r\n');
      if (marker < 0) return;
      const headerText = this.handshakeBuffer.subarray(0, marker).toString('utf8');
      const remainder = this.handshakeBuffer.subarray(marker + 4);
      if (!this.isValidHandshake(headerText)) {
        this.finish('League Client rejected the event connection.');
        return;
      }
      this.handshakeComplete = true;
      this.handshakeBuffer = Buffer.alloc(0);
      this.socket?.setTimeout(0);
      this.emitState({connected: true, connecting: false, message: 'League Client connected.'});
      this.sendFrame(Buffer.from(JSON.stringify([5, 'OnJsonApiEvent']), 'utf8'), 1);
      if (remainder.length) this.receiveFrames(remainder);
      return;
    }
    this.receiveFrames(chunk);
  }

  private isValidHandshake(headerText: string): boolean {
    const lines = headerText.split('\r\n');
    if (!/^HTTP\/1\.[01] 101\b/.test(lines[0] || '')) return false;
    const headers = new Map<string, string>();
    for (const line of lines.slice(1)) {
      const separator = line.indexOf(':');
      if (separator <= 0) continue;
      headers.set(line.slice(0, separator).trim().toLowerCase(), line.slice(separator + 1).trim());
    }
    const expectedAccept = createHash('sha1').update(`${this.key}${WEBSOCKET_GUID}`).digest('base64');
    return headers.get('upgrade')?.toLowerCase() === 'websocket'
      && headers.get('connection')?.toLowerCase().split(/\s*,\s*/).includes('upgrade') === true
      && headers.get('sec-websocket-accept') === expectedAccept;
  }

  private receiveFrames(chunk: Buffer): void {
    try {
      for (const frame of this.decoder.push(chunk)) {
        if (frame.type === 'ping') {
          this.sendFrame(frame.payload, 10);
        } else if (frame.type === 'close') {
          this.finish('League Client event connection closed.');
          return;
        } else {
          this.handleText(frame.payload);
        }
      }
    } catch {
      this.finish('League Client sent an invalid event message.');
    }
  }

  private handleText(text: string): void {
    try {
      const message = JSON.parse(text) as unknown;
      if (!Array.isArray(message) || message.length < 3) return;
      const value = message[2] as Partial<LcuJsonApiEvent> | null;
      if (!value || typeof value.uri !== 'string') return;
      this.emitEvent({
        uri: value.uri,
        eventType: typeof value.eventType === 'string' ? value.eventType : '',
        data: value.data
      });
    } catch {
      return;
    }
  }

  private sendFrame(payload: Buffer, opcode: number): void {
    if (!this.socket || this.socket.destroyed) return;
    const mask = randomBytes(4);
    let header: Buffer;
    if (payload.length < 126) {
      header = Buffer.from([0x80 | opcode, 0x80 | payload.length]);
    } else if (payload.length < 65536) {
      header = Buffer.alloc(4);
      header[0] = 0x80 | opcode;
      header[1] = 0x80 | 126;
      header.writeUInt16BE(payload.length, 2);
    } else {
      header = Buffer.alloc(10);
      header[0] = 0x80 | opcode;
      header[1] = 0x80 | 127;
      header.writeUInt32BE(0, 2);
      header.writeUInt32BE(payload.length, 6);
    }
    const masked = Buffer.alloc(payload.length);
    for (let index = 0; index < payload.length; index++) {
      masked[index] = (payload[index] ?? 0) ^ (mask[index % 4] ?? 0);
    }
    this.socket.write(Buffer.concat([header, mask, masked]));
  }

  private finish(message: string): void {
    if (this.closed) return;
    this.close();
    this.emitState({connected: false, connecting: false, message});
  }
}
