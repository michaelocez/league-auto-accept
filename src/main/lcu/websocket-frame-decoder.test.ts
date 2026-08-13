import assert from 'node:assert/strict';
import {describe, it} from 'node:test';
import {WebSocketFrameDecoder} from './websocket-frame-decoder';

function serverFrame(opcode: number, payloadValue: string, finished = true): Buffer {
  const payload = Buffer.from(payloadValue, 'utf8');
  assert.ok(payload.length < 126);
  return Buffer.concat([
    Buffer.from([(finished ? 0x80 : 0) | opcode, payload.length]),
    payload
  ]);
}

describe('WebSocketFrameDecoder', () => {
  it('decodes complete and fragmented text messages', () => {
    const decoder = new WebSocketFrameDecoder(1024);
    assert.deepEqual(decoder.push(serverFrame(1, 'ready')), [{type: 'text', payload: 'ready'}]);
    assert.deepEqual(decoder.push(serverFrame(1, 'queue ', false)), []);
    assert.deepEqual(decoder.push(serverFrame(0, 'popped')), [{type: 'text', payload: 'queue popped'}]);
  });

  it('surfaces ping frames and rejects masked server frames', () => {
    const decoder = new WebSocketFrameDecoder(1024);
    const ping = decoder.push(serverFrame(9, 'hello'));
    assert.equal(ping[0]?.type, 'ping');
    assert.throws(() => decoder.push(Buffer.from([0x81, 0x80, 0, 0, 0, 0])), /masked frame/);
  });

  it('enforces the configured payload limit', () => {
    const decoder = new WebSocketFrameDecoder(4);
    assert.throws(() => decoder.push(serverFrame(1, '12345')), /size limit/);
  });
});
