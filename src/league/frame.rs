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

    /// Builds a server frame using the 16-bit or 64-bit extended length when required.
    fn server_frame_ext(opcode: u8, payload: &[u8], finished: bool) -> Vec<u8> {
        let mut out = vec![(if finished { 0x80 } else { 0 }) | opcode];
        if payload.len() < 126 {
            out.push(payload.len() as u8);
        } else if payload.len() < 65536 {
            out.push(126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            out.push(127);
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        }
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
    fn decodes_extended_payload_lengths() {
        let mut decoder = WebSocketFrameDecoder::new(1 << 20);
        let medium = vec![b'a'; 200];
        assert_eq!(
            decoder.push(&server_frame_ext(0x1, &medium, true)).unwrap(),
            vec![DecodedFrame::Text("a".repeat(200))]
        );
        let large = vec![b'b'; 70_000];
        assert_eq!(
            decoder.push(&server_frame_ext(0x1, &large, true)).unwrap(),
            vec![DecodedFrame::Text("b".repeat(70_000))]
        );
    }

    #[test]
    fn returns_every_frame_decoded_from_one_push() {
        let mut decoder = WebSocketFrameDecoder::new(1024);
        let mut chunk = server_frame(0x1, b"first", true);
        chunk.extend_from_slice(&server_frame(0x1, b"second", true));
        assert_eq!(
            decoder.push(&chunk).unwrap(),
            vec![
                DecodedFrame::Text("first".into()),
                DecodedFrame::Text("second".into())
            ]
        );
    }

    #[test]
    fn surfaces_close_and_ignores_pong() {
        let mut decoder = WebSocketFrameDecoder::new(1024);
        assert_eq!(
            decoder
                .push(&server_frame(0x8, &[0x03, 0xE8], true))
                .unwrap(),
            vec![DecodedFrame::Close(vec![0x03, 0xE8])]
        );
        assert_eq!(
            decoder.push(&server_frame(0xA, b"pong", true)).unwrap(),
            vec![]
        );
    }

    #[test]
    fn rejects_reserved_bits_and_unknown_opcodes() {
        // A rejected frame leaves the decoder's buffer untouched, so use a fresh decoder per case.
        assert_eq!(
            WebSocketFrameDecoder::new(1024).push(&[0xC1, 0x00]),
            Err(FrameError::ReservedBits)
        );
        assert_eq!(
            WebSocketFrameDecoder::new(1024).push(&[0x83, 0x00]),
            Err(FrameError::UnsupportedOpcode)
        );
    }

    #[test]
    fn rejects_invalid_control_frames() {
        let mut decoder = WebSocketFrameDecoder::new(1024);
        // A control frame must not be fragmented.
        assert_eq!(
            decoder.push(&server_frame(0x9, b"hi", false)),
            Err(FrameError::InvalidControlFrame)
        );
    }

    #[test]
    fn rejects_continuations_without_a_message_and_interleaved_messages() {
        let mut decoder = WebSocketFrameDecoder::new(1024);
        assert_eq!(
            decoder.push(&server_frame(0x0, b"orphan", true)),
            Err(FrameError::UnexpectedContinuation)
        );

        let mut decoder = WebSocketFrameDecoder::new(1024);
        assert_eq!(
            decoder.push(&server_frame(0x1, b"start", false)).unwrap(),
            vec![]
        );
        assert_eq!(
            decoder.push(&server_frame(0x1, b"again", true)),
            Err(FrameError::InterleavedMessage)
        );
    }

    #[test]
    fn rejects_a_buffer_that_outgrows_its_limit_without_a_complete_frame() {
        let mut decoder = WebSocketFrameDecoder::new(4);
        // A frame header claiming 16 payload bytes that never arrive, padded past the buffer cap.
        let mut chunk = vec![0x81, 0x7E, 0x00, 0x10];
        chunk.extend_from_slice(&[0u8; 15]);
        assert_eq!(decoder.push(&chunk), Err(FrameError::BufferExceeded));
    }

    #[test]
    fn rejects_a_fragmented_message_that_outgrows_the_limit() {
        let mut decoder = WebSocketFrameDecoder::new(4);
        assert_eq!(
            decoder.push(&server_frame(0x1, b"abc", false)).unwrap(),
            vec![]
        );
        assert_eq!(
            decoder.push(&server_frame(0x0, b"de", false)),
            Err(FrameError::FragmentedMessageExceeded)
        );
    }

    #[test]
    fn rejects_a_frame_whose_64_bit_length_exceeds_the_limit() {
        let mut decoder = WebSocketFrameDecoder::new(1024);
        // 64-bit length with a non-zero high word is rejected before any payload is read.
        let mut chunk = vec![0x81, 0x7F];
        chunk.extend_from_slice(&1u32.to_be_bytes());
        chunk.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(decoder.push(&chunk), Err(FrameError::FrameTooLarge));
    }

    #[test]
    fn encodes_a_masked_client_frame() {
        let frame = encode_masked(0x1, b"hello", [0, 0, 0, 0]);
        assert_eq!(
            frame,
            vec![0x81, 0x85, 0, 0, 0, 0, b'h', b'e', b'l', b'l', b'o']
        );
        let masked = encode_masked(0x1, b"hi", [1, 2, 3, 4]);
        assert_eq!(masked[1] & 0x80, 0x80);
        assert_eq!(masked[6], b'h' ^ 1);
        assert_eq!(masked[7], b'i' ^ 2);
    }
}
