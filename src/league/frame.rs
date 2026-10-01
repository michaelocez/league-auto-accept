//! WebSocket frame decoding for LCU events.
//!
//! Server frames are unmasked; masked server frames, reserved bits, invalid control frames and
//! unknown opcodes are rejected.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodedFrame {
    Text(String),
    Ping(Vec<u8>),
    Close(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    BufferExceeded,
    FragmentedMessageExceeded,
    ReservedBits,
    MaskedServerFrame,
    FrameTooLarge,
    InvalidControlFrame,
    InterleavedMessage,
    UnexpectedContinuation,
    UnsupportedOpcode,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            FrameError::BufferExceeded => "LCU event buffer exceeded its size limit",
            FrameError::FragmentedMessageExceeded => {
                "LCU fragmented message exceeded its size limit"
            }
            FrameError::ReservedBits => "LCU event frame used unsupported extensions",
            FrameError::MaskedServerFrame => "LCU event server sent a masked frame",
            FrameError::FrameTooLarge => "LCU event frame exceeded the supported size limit",
            FrameError::InvalidControlFrame => "LCU sent an invalid control frame",
            FrameError::InterleavedMessage => {
                "LCU started a new message before completing the previous one"
            }
            FrameError::UnexpectedContinuation => "LCU sent an unexpected continuation frame",
            FrameError::UnsupportedOpcode => "LCU sent an unsupported WebSocket frame",
        };
        write!(f, "{message}")
    }
}

impl std::error::Error for FrameError {}

pub struct WebSocketFrameDecoder {
    buffer: Vec<u8>,
    fragmented: Option<Vec<u8>>,
    fragmented_len: usize,
    max_payload_bytes: usize,
}

impl WebSocketFrameDecoder {
    pub fn new(max_payload_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            fragmented: None,
            fragmented_len: 0,
            max_payload_bytes,
        }
    }

    /// Pushes received bytes and returns any complete frames.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<DecodedFrame>, FrameError> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > self.max_payload_bytes + 14 {
            return Err(FrameError::BufferExceeded);
        }
        let mut frames = Vec::new();

        loop {
            if self.buffer.len() < 2 {
                break;
            }
            let first = self.buffer[0];
            let second = self.buffer[1];
            let finished = first & 0x80 != 0;
            let reserved = first & 0x70;
            let opcode = first & 0x0f;
            let masked = second & 0x80 != 0;
            let mut payload_len = (second & 0x7f) as usize;
            let mut offset = 2;

            if reserved != 0 {
                return Err(FrameError::ReservedBits);
            }
            if masked {
                return Err(FrameError::MaskedServerFrame);
            }
            if payload_len == 126 {
                if self.buffer.len() < 4 {
                    break;
                }
                payload_len = u16::from_be_bytes([self.buffer[2], self.buffer[3]]) as usize;
                offset = 4;
            } else if payload_len == 127 {
                if self.buffer.len() < 10 {
                    break;
                }
                let high = u32::from_be_bytes([
                    self.buffer[2],
                    self.buffer[3],
                    self.buffer[4],
                    self.buffer[5],
                ]);
                let low = u32::from_be_bytes([
                    self.buffer[6],
                    self.buffer[7],
                    self.buffer[8],
                    self.buffer[9],
                ]);
                if high != 0 {
                    return Err(FrameError::FrameTooLarge);
                }
                payload_len = low as usize;
                offset = 10;
            }
            if payload_len > self.max_payload_bytes {
                return Err(FrameError::FrameTooLarge);
            }
            if self.buffer.len() < offset + payload_len {
                break;
            }

            let payload = self.buffer[offset..offset + payload_len].to_vec();
            self.buffer.drain(..offset + payload_len);

            let control = opcode >= 8;
            if control && (!finished || payload_len > 125) {
                return Err(FrameError::InvalidControlFrame);
            }

            match opcode {
                0x8 => frames.push(DecodedFrame::Close(payload)),
                0x9 => frames.push(DecodedFrame::Ping(payload)),
                0xA => {}
                0x1 => {
                    if self.fragmented.is_some() {
                        return Err(FrameError::InterleavedMessage);
                    }
                    if finished {
                        frames.push(DecodedFrame::Text(
                            String::from_utf8_lossy(&payload).into_owned(),
                        ));
                    } else {
                        self.fragmented_len = payload.len();
                        self.fragmented = Some(payload);
                    }
                }
                0x0 => {
                    let Some(buffer) = self.fragmented.as_mut() else {
                        return Err(FrameError::UnexpectedContinuation);
                    };
                    self.fragmented_len += payload.len();
                    if self.fragmented_len > self.max_payload_bytes {
                        return Err(FrameError::FragmentedMessageExceeded);
                    }
                    buffer.extend_from_slice(&payload);
                    if finished {
                        let complete = self.fragmented.take().unwrap_or_default();
                        self.fragmented_len = 0;
                        frames.push(DecodedFrame::Text(
                            String::from_utf8_lossy(&complete).into_owned(),
                        ));
                    }
                }
                _ => return Err(FrameError::UnsupportedOpcode),
            }
        }
        Ok(frames)
    }
}

/// Encodes a masked client frame (client frames must be masked).
pub fn encode_masked(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    if payload.len() < 126 {
        out.push(0x80 | payload.len() as u8);
    } else if payload.len() < 65536 {
        out.push(0x80 | 126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    for (index, byte) in payload.iter().enumerate() {
        out.push(byte ^ mask[index % 4]);
    }
    out
}

/// Encodes a masked client text frame (the LCU subscription is the only client text frame).
pub fn encode_masked_text(payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    encode_masked(0x1, payload, mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_frame(opcode: u8, payload: &[u8], finished: bool) -> Vec<u8> {
        assert!(payload.len() < 126);
        let mut out = vec![
            (if finished { 0x80 } else { 0 }) | opcode,
            payload.len() as u8,
        ];
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn decodes_complete_and_fragmented_text_messages() {
        let mut decoder = WebSocketFrameDecoder::new(1024);
        assert_eq!(
            decoder.push(&server_frame(0x1, b"ready", true)).unwrap(),
            vec![DecodedFrame::Text("ready".into())]
        );
        assert_eq!(
            decoder.push(&server_frame(0x1, b"queue ", false)).unwrap(),
            vec![]
        );
        assert_eq!(
            decoder.push(&server_frame(0x0, b"popped", true)).unwrap(),
            vec![DecodedFrame::Text("queue popped".into())]
        );
    }

    #[test]
    fn surfaces_ping_and_rejects_masked_server_frames() {
        let mut decoder = WebSocketFrameDecoder::new(1024);
        assert_eq!(
            decoder.push(&server_frame(0x9, b"hi", true)).unwrap(),
            vec![DecodedFrame::Ping(b"hi".to_vec())]
        );
        assert_eq!(
            decoder.push(&[0x81, 0x80, 0, 0, 0, 0]),
            Err(FrameError::MaskedServerFrame)
        );
    }

    #[test]
    fn enforces_the_payload_limit() {
        let mut decoder = WebSocketFrameDecoder::new(4);
        assert_eq!(
            decoder.push(&server_frame(0x1, b"12345", true)),
            Err(FrameError::FrameTooLarge)
        );
    }

    #[test]
    fn encodes_a_masked_client_frame() {
        let frame = encode_masked_text(b"hello", [0, 0, 0, 0]);
        assert_eq!(
            frame,
            vec![0x81, 0x85, 0, 0, 0, 0, b'h', b'e', b'l', b'l', b'o']
        );
        let masked = encode_masked_text(b"hi", [1, 2, 3, 4]);
        assert_eq!(masked[1] & 0x80, 0x80);
        assert_eq!(masked[6], b'h' ^ 1);
        assert_eq!(masked[7], b'i' ^ 2);
    }
}
