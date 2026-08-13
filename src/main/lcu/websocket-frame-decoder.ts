export type DecodedWebSocketFrame =
  | {type: 'text'; payload: string}
  | {type: 'ping'; payload: Buffer}
  | {type: 'close'; payload: Buffer};

export class WebSocketFrameDecoder {
  private buffer = Buffer.alloc(0);
  private fragmentedText: Buffer[] | null = null;
  private fragmentedBytes = 0;

  constructor(private readonly maxPayloadBytes: number) {
  }

  public push(chunk: Buffer): DecodedWebSocketFrame[] {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    if (this.buffer.length > this.maxPayloadBytes + 14) throw new Error('LCU event buffer exceeded its size limit.');
    const frames: DecodedWebSocketFrame[] = [];

    while (this.buffer.length >= 2) {
      const first = this.buffer[0] ?? 0;
      const second = this.buffer[1] ?? 0;
      const finished = (first & 0x80) !== 0;
      const reserved = first & 0x70;
      const opcode = first & 0x0f;
      const masked = (second & 0x80) !== 0;
      let payloadLength = second & 0x7f;
      let offset = 2;

      if (reserved) throw new Error('LCU event frame used unsupported extensions.');
      if (masked) throw new Error('LCU event server sent a masked frame.');
      if (payloadLength === 126) {
        if (this.buffer.length < 4) break;
        payloadLength = this.buffer.readUInt16BE(2);
        offset = 4;
      } else if (payloadLength === 127) {
        if (this.buffer.length < 10) break;
        const high = this.buffer.readUInt32BE(2);
        const low = this.buffer.readUInt32BE(6);
        if (high !== 0) throw new Error('LCU event frame exceeded the supported size limit.');
        payloadLength = low;
        offset = 10;
      }
      if (payloadLength > this.maxPayloadBytes) throw new Error('LCU event frame exceeded the supported size limit.');
      if (this.buffer.length < offset + payloadLength) break;

      const payload = Buffer.from(this.buffer.subarray(offset, offset + payloadLength));
      this.buffer = this.buffer.subarray(offset + payloadLength);
      const controlFrame = opcode >= 8;
      if (controlFrame && (!finished || payloadLength > 125)) throw new Error('LCU sent an invalid control frame.');

      if (opcode === 8) {
        frames.push({type: 'close', payload});
      } else if (opcode === 9) {
        frames.push({type: 'ping', payload});
      } else if (opcode === 10) {
        continue;
      } else if (opcode === 1) {
        if (this.fragmentedText) throw new Error('LCU started a new message before completing the previous one.');
        if (finished) {
          frames.push({type: 'text', payload: payload.toString('utf8')});
        } else {
          this.fragmentedText = [payload];
          this.fragmentedBytes = payload.length;
        }
      } else if (opcode === 0) {
        if (!this.fragmentedText) throw new Error('LCU sent an unexpected continuation frame.');
        this.fragmentedBytes += payload.length;
        if (this.fragmentedBytes > this.maxPayloadBytes) throw new Error('LCU fragmented message exceeded its size limit.');
        this.fragmentedText.push(payload);
        if (finished) {
          frames.push({type: 'text', payload: Buffer.concat(this.fragmentedText).toString('utf8')});
          this.fragmentedText = null;
          this.fragmentedBytes = 0;
        }
      } else {
        throw new Error('LCU sent an unsupported WebSocket frame.');
      }
    }
    return frames;
  }
}
