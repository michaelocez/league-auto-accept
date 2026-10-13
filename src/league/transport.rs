//! Concrete LCU transports: loopback HTTPS REST and a strict WebSocket event socket.
//!
//! The LCU serves a self-signed certificate on `127.0.0.1`, so certificate verification is
//! intentionally disabled — but only for this loopback connection; authentication is still
//! required via HTTP Basic and the WebSocket handshake is verified.
//!
//! The WebSocket path uses the project's strict `WebSocketFrameDecoder` rather than a generic
//! client, which gives explicit malformed-frame handling.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use rand::RngCore;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::api::{HttpMethod, LcuRequest, LcuRequestError};
use super::frame::{encode_masked, DecodedFrame, WebSocketFrameDecoder};
use super::lockfile::LcuCredentials;
use super::{
    EVENT_SUBSCRIPTION_FRAME, GAMEFLOW_ENDPOINT, READY_CHECK_ACCEPT_ENDPOINT, READY_CHECK_ENDPOINT,
    SUMMONER_ENDPOINT,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
/// How long the event socket may stay silent before the client sends a keepalive ping.
const EVENT_IDLE_PING: Duration = Duration::from_secs(30);
/// How long the client waits for any data after a keepalive ping before treating the socket as
/// half-open and reporting a timeout, so the service can reconnect.
const EVENT_PONG_TIMEOUT: Duration = Duration::from_secs(10);
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug)]
pub enum TransportError {
    Io(String),
    Tls(String),
    Timeout,
    Http { status: u16 },
    Protocol(String),
    Json(String),
    Request(LcuRequestError),
    Handshake(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Io(error) => write!(f, "LCU I/O error: {error}"),
            TransportError::Tls(error) => write!(f, "LCU TLS error: {error}"),
            TransportError::Timeout => write!(f, "LCU request timed out"),
            TransportError::Http { status } => write!(f, "LCU returned HTTP {status}"),
            TransportError::Protocol(error) => write!(f, "LCU protocol error: {error}"),
            TransportError::Json(error) => write!(f, "LCU returned invalid JSON: {error}"),
            TransportError::Request(error) => write!(f, "{error}"),
            TransportError::Handshake(error) => write!(f, "LCU handshake failed: {error}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Certificate verifier that accepts any certificate. Only ever used for the LCU loopback
/// connection, which serves a self-signed certificate.
#[derive(Debug)]
struct AcceptAnyCertificate {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn tls_config() -> Result<Arc<ClientConfig>, TransportError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| TransportError::Tls(error.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate { provider }))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Loopback HTTPS REST client for the LCU.
///
/// The public surface is the closed set of calls the application makes; the generic
/// `request`/`get` helpers are private and endpoints are allowlisted in `league::api`.
pub struct LcuRestClient {
    credentials: LcuCredentials,
    /// TLS setup is fallible; a failure is surfaced on the first request rather than panicking.
    config: Result<Arc<ClientConfig>, String>,
}

impl LcuRestClient {
    pub fn new(credentials: LcuCredentials) -> Self {
        Self {
            credentials,
            config: tls_config().map_err(|error| error.to_string()),
        }
    }

    /// `GET /lol-summoner/v1/current-summoner` — health/authentication check.
    pub async fn verify(&self) -> Result<(), TransportError> {
        self.get(SUMMONER_ENDPOINT).await.map(|_| ())
    }

    pub async fn get_gameflow_phase(&self) -> Result<String, TransportError> {
        let value = self.get(GAMEFLOW_ENDPOINT).await?;
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| TransportError::Protocol("gameflow phase was not a string".into()))
    }

    pub async fn get_ready_check(&self) -> Result<serde_json::Value, TransportError> {
        self.get(READY_CHECK_ENDPOINT).await
    }

    /// `POST /lol-matchmaking/v1/ready-check/accept`.
    pub async fn accept(&self) -> Result<(), TransportError> {
        let request = LcuRequest::new(HttpMethod::Post, READY_CHECK_ACCEPT_ENDPOINT, None)
            .map_err(TransportError::Request)?;
        self.request(&request).await.map(|_| ())
    }

    async fn get(&self, endpoint: &str) -> Result<serde_json::Value, TransportError> {
        let request =
            LcuRequest::new(HttpMethod::Get, endpoint, None).map_err(TransportError::Request)?;
        self.request(&request).await
    }

    async fn request(&self, request: &LcuRequest) -> Result<serde_json::Value, TransportError> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.request_inner(request))
            .await
            .map_err(|_| TransportError::Timeout)?
    }

    async fn request_inner(
        &self,
        request: &LcuRequest,
    ) -> Result<serde_json::Value, TransportError> {
        let config = self
            .config
            .as_ref()
            .map_err(|error| TransportError::Tls(error.clone()))?
            .clone();
        let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, self.credentials.port))
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;
        let connector = TlsConnector::from(config);
        let server_name = ServerName::try_from("127.0.0.1")
            .map_err(|error| TransportError::Tls(error.to_string()))?;
        let mut tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|error| TransportError::Tls(error.to_string()))?;

        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAccept: application/json\r\nAuthorization: {}\r\nConnection: close\r\n",
            request.method.as_str(),
            request.endpoint,
            self.credentials.port,
            self.credentials.authorization(),
        );
        if let Some(body) = &request.body {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        head.push_str("\r\n");

        let mut bytes = head.into_bytes();
        if let Some(body) = &request.body {
            bytes.extend_from_slice(body.as_bytes());
        }
        tls.write_all(&bytes)
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;
        tls.flush()
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;

        let mut response = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match tls.read(&mut chunk).await {
                Ok(0) => break,
                Ok(read) => {
                    response.extend_from_slice(&chunk[..read]);
                    if response.len() > MAX_RESPONSE_BYTES {
                        return Err(TransportError::Protocol(
                            "LCU response exceeded the supported size limit".into(),
                        ));
                    }
                }
                // The LCU commonly closes the connection without a TLS close_notify. Once the
                // full response head has arrived, treat a read error as end-of-stream.
                Err(error) => {
                    if find_double_crlf(&response).is_some() {
                        break;
                    }
                    return Err(TransportError::Io(error.to_string()));
                }
            }
        }

        let separator = find_double_crlf(&response)
            .ok_or_else(|| TransportError::Protocol("malformed HTTP response".into()))?;
        let head = String::from_utf8_lossy(&response[..separator]);
        let status = parse_status(&head)
            .ok_or_else(|| TransportError::Protocol("missing HTTP status".into()))?;
        if !(200..300).contains(&status) {
            return Err(TransportError::Http { status });
        }
        let body = &response[separator + 4..];
        // A body shorter than its declared `Content-Length` means the response was truncated;
        // parsing it anyway could turn a failed accept into an apparent success (or vice versa).
        let body = match content_length(&head) {
            Some(expected) if body.len() < expected => {
                return Err(TransportError::Protocol(
                    "truncated HTTP response body".into(),
                ));
            }
            Some(expected) => &body[..expected],
            None => body,
        };
        if body.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(body).map_err(|error| TransportError::Json(error.to_string()))
    }
}

/// Loopback TLS WebSocket client for LCU events.
pub struct LcuEventSocket {
    credentials: LcuCredentials,
    /// TLS setup is fallible; a failure is surfaced on connect rather than panicking.
    config: Result<Arc<ClientConfig>, String>,
}

impl LcuEventSocket {
    pub fn new(credentials: LcuCredentials) -> Self {
        Self {
            credentials,
            config: tls_config().map_err(|error| error.to_string()),
        }
    }

    pub async fn connect(&self) -> Result<LcuEventConnection, TransportError> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, self.connect_inner())
            .await
            .map_err(|_| TransportError::Timeout)?
    }

    async fn connect_inner(&self) -> Result<LcuEventConnection, TransportError> {
        let config = self
            .config
            .as_ref()
            .map_err(|error| TransportError::Tls(error.clone()))?
            .clone();
        let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, self.credentials.port))
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;
        tcp.set_nodelay(true)
            .map_err(|error| TransportError::Io(error.to_string()))?;
        let connector = TlsConnector::from(config);
        let server_name = ServerName::try_from("127.0.0.1")
            .map_err(|error| TransportError::Tls(error.to_string()))?;
        let mut tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|error| TransportError::Tls(error.to_string()))?;

        let mut key_bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut key_bytes);
        let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
        let handshake = format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\nAuthorization: {}\r\n\r\n",
            self.credentials.port,
            key,
            self.credentials.authorization(),
        );
        tls.write_all(handshake.as_bytes())
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;
        tls.flush()
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;

        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let separator = loop {
            if let Some(position) = find_double_crlf(&buffer) {
                break position;
            }
            let read = tls
                .read(&mut chunk)
                .await
                .map_err(|error| TransportError::Io(error.to_string()))?;
            if read == 0 {
                return Err(TransportError::Handshake(
                    "connection closed during handshake".into(),
                ));
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.len() > MAX_HANDSHAKE_BYTES {
                return Err(TransportError::Handshake(
                    "handshake exceeded its size limit".into(),
                ));
            }
        };

        let header = String::from_utf8_lossy(&buffer[..separator]).to_string();
        let remainder = buffer[separator + 4..].to_vec();
        validate_handshake(&header, &key)?;

        let mut connection = LcuEventConnection {
            tls,
            decoder: WebSocketFrameDecoder::new(MAX_EVENT_BYTES),
            leftover: remainder,
            queue: SocketQueue::default(),
            ping_outstanding: false,
        };
        connection
            .send_frame(0x1, EVENT_SUBSCRIPTION_FRAME.as_bytes())
            .await?;
        Ok(connection)
    }
}

fn validate_handshake(header: &str, key: &str) -> Result<(), TransportError> {
    let mut lines = header.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    if !status_line.starts_with("HTTP/1.1 101") && !status_line.starts_with("HTTP/1.0 101") {
        return Err(TransportError::Handshake(format!(
            "unexpected status line: {status_line}"
        )));
    }
    let mut upgrade = String::new();
    let mut connection = String::new();
    let mut accept = String::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "upgrade" => upgrade = value.to_ascii_lowercase(),
            "connection" => connection = value.to_ascii_lowercase(),
            "sec-websocket-accept" => accept = value.to_string(),
            _ => {}
        }
    }
    if upgrade != "websocket" || !connection.split(',').any(|token| token.trim() == "upgrade") {
        return Err(TransportError::Handshake(
            "missing WebSocket upgrade headers".into(),
        ));
    }
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    let expected = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
    if accept != expected {
        return Err(TransportError::Handshake(
            "Sec-WebSocket-Accept mismatch".into(),
        ));
    }
    Ok(())
}

/// Events surfaced by the event socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketEvent {
    Text(String),
    Closed,
}

/// Decoded frames from one read, delivered one at a time.
///
/// A single TLS read can carry several events; returning only the first would silently drop the
/// rest, including a ready check, so the remainder is queued instead.
#[derive(Default)]
struct SocketQueue {
    queued: VecDeque<SocketEvent>,
    closed: bool,
}

impl SocketQueue {
    /// Converts decoded frames into deliverable events and returns the ping payloads that must
    /// be answered with a pong. Frames after a close are discarded.
    fn push(&mut self, frames: Vec<DecodedFrame>) -> Vec<Vec<u8>> {
        let mut pongs = Vec::new();
        for frame in frames {
            if self.closed {
                break;
            }
            match frame {
                DecodedFrame::Text(text) => self.queued.push_back(SocketEvent::Text(text)),
                DecodedFrame::Close(_) => {
                    self.closed = true;
                    self.queued.push_back(SocketEvent::Closed);
                }
                DecodedFrame::Ping(payload) => pongs.push(payload),
            }
        }
        pongs
    }

    fn pop(&mut self) -> Option<SocketEvent> {
        self.queued.pop_front()
    }

    fn is_closed(&self) -> bool {
        self.closed
    }
}

/// A connected LCU event socket.
pub struct LcuEventConnection {
    tls: tokio_rustls::client::TlsStream<TcpStream>,
    decoder: WebSocketFrameDecoder,
    leftover: Vec<u8>,
    queue: SocketQueue,
    ping_outstanding: bool,
}

impl LcuEventConnection {
    async fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), TransportError> {
        let mut mask = [0u8; 4];
        rand::rng().fill_bytes(&mut mask);
        let frame = encode_masked(opcode, payload, mask);
        self.tls
            .write_all(&frame)
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;
        self.tls
            .flush()
            .await
            .map_err(|error| TransportError::Io(error.to_string()))
    }

    /// Returns the next text event, or `Closed` when the connection ends.
    ///
    /// Frames decoded from one read are queued and returned one per call. Ping frames are
    /// answered with a pong and do not surface. When the socket stays silent for
    /// `EVENT_IDLE_PING` a keepalive ping is sent; if it stays silent for `EVENT_PONG_TIMEOUT`
    /// afterwards the connection is treated as dead so the service can reconnect.
    pub async fn next_event(&mut self) -> Result<SocketEvent, TransportError> {
        loop {
            if let Some(event) = self.queue.pop() {
                return Ok(event);
            }
            if self.queue.is_closed() {
                return Ok(SocketEvent::Closed);
            }

            let chunk = if self.leftover.is_empty() {
                let mut buffer = [0u8; 8192];
                let wait = if self.ping_outstanding {
                    EVENT_PONG_TIMEOUT
                } else {
                    EVENT_IDLE_PING
                };
                match tokio::time::timeout(wait, self.tls.read(&mut buffer)).await {
                    Ok(Ok(0)) => return Ok(SocketEvent::Closed),
                    Ok(Ok(read)) => {
                        self.ping_outstanding = false;
                        buffer[..read].to_vec()
                    }
                    Ok(Err(error)) => return Err(TransportError::Io(error.to_string())),
                    Err(_) if self.ping_outstanding => return Err(TransportError::Timeout),
                    Err(_) => {
                        self.send_frame(0x9, &[]).await?;
                        self.ping_outstanding = true;
                        continue;
                    }
                }
            } else {
                self.ping_outstanding = false;
                std::mem::take(&mut self.leftover)
            };

            let frames = self
                .decoder
                .push(&chunk)
                .map_err(|error| TransportError::Protocol(error.to_string()))?;
            let pongs = self.queue.push(frames);
            for payload in pongs {
                self.send_frame(0xA, &payload).await?;
            }
        }
    }
}

fn find_double_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Parses the `Content-Length` header, which LCU responses always carry.
fn content_length(header: &str) -> Option<usize> {
    header.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
}

fn parse_status(header: &str) -> Option<u16> {
    let line = header.lines().next()?;
    let mut parts = line.split_whitespace();
    parts.next()?; // HTTP/1.1
    parts.next()?.parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

    fn accept_for(key: &str) -> String {
        let mut hasher = Sha1::new();
        hasher.update(key.as_bytes());
        hasher.update(WEBSOCKET_GUID.as_bytes());
        base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
    }

    fn handshake_header(key: &str) -> String {
        format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n",
            accept_for(key)
        )
    }

    #[test]
    fn accepts_a_valid_handshake() {
        assert!(validate_handshake(&handshake_header(KEY), KEY).is_ok());
    }

    #[test]
    fn accepts_header_name_case_and_connection_token_lists() {
        let header = format!(
            "HTTP/1.0 101 Switching Protocols\r\nUPGRADE: WebSocket\r\nconnection: keep-alive, Upgrade\r\nsec-websocket-accept: {}\r\n",
            accept_for(KEY)
        );
        assert!(validate_handshake(&header, KEY).is_ok());
    }

    #[test]
    fn rejects_a_status_line_that_is_not_101() {
        let header = format!(
            "HTTP/1.1 200 OK\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n",
            accept_for(KEY)
        );
        match validate_handshake(&header, KEY) {
            Err(TransportError::Handshake(message)) => {
                assert!(message.contains("status line"), "unexpected: {message}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_upgrade_headers_that_are_missing_or_wrong() {
        let missing = format!(
            "HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: {}\r\n",
            accept_for(KEY)
        );
        assert!(matches!(
            validate_handshake(&missing, KEY),
            Err(TransportError::Handshake(_))
        ));

        let header = handshake_header(KEY).replace("Upgrade: websocket", "Upgrade: h2c");
        assert!(matches!(
            validate_handshake(&header, KEY),
            Err(TransportError::Handshake(_))
        ));
    }

    #[test]
    fn rejects_a_mismatched_accept_value() {
        match validate_handshake(&handshake_header("some-other-key"), KEY) {
            Err(TransportError::Handshake(message)) => {
                assert!(message.contains("mismatch"), "unexpected: {message}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parses_the_status_line() {
        assert_eq!(parse_status("HTTP/1.1 204 No Content"), Some(204));
        assert_eq!(parse_status("HTTP/1.1 404 Not Found"), Some(404));
        assert_eq!(parse_status("garbage"), None);
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn parses_content_length_case_insensitively() {
        assert_eq!(
            content_length("HTTP/1.1 200 OK\r\nContent-Length: 42\r\n"),
            Some(42)
        );
        assert_eq!(
            content_length("HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n"),
            Some(0)
        );
        assert_eq!(content_length("HTTP/1.1 200 OK\r\nConnection: close"), None);
        assert_eq!(
            content_length("HTTP/1.1 200 OK\r\nContent-Length: not-a-number\r\n"),
            None
        );
    }

    #[test]
    fn queues_every_frame_decoded_from_one_read() {
        let mut queue = SocketQueue::default();
        let pongs = queue.push(vec![
            DecodedFrame::Text("first".into()),
            DecodedFrame::Text("second".into()),
            DecodedFrame::Text("third".into()),
        ]);
        assert!(pongs.is_empty());
        assert_eq!(queue.pop(), Some(SocketEvent::Text("first".into())));
        assert_eq!(queue.pop(), Some(SocketEvent::Text("second".into())));
        assert_eq!(queue.pop(), Some(SocketEvent::Text("third".into())));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn answers_pings_without_surfacing_them() {
        let mut queue = SocketQueue::default();
        let pongs = queue.push(vec![
            DecodedFrame::Ping(b"keepalive".to_vec()),
            DecodedFrame::Text("event".into()),
        ]);
        assert_eq!(pongs, vec![b"keepalive".to_vec()]);
        assert_eq!(queue.pop(), Some(SocketEvent::Text("event".into())));
    }

    #[test]
    fn a_close_ends_the_stream_and_discards_later_frames() {
        let mut queue = SocketQueue::default();
        queue.push(vec![
            DecodedFrame::Text("before".into()),
            DecodedFrame::Close(vec![]),
            DecodedFrame::Text("after".into()),
        ]);
        assert_eq!(queue.pop(), Some(SocketEvent::Text("before".into())));
        assert_eq!(queue.pop(), Some(SocketEvent::Closed));
        assert_eq!(queue.pop(), None);
        assert!(queue.is_closed());
    }
}
