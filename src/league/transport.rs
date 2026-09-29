//! Concrete LCU transports: loopback HTTPS REST and a strict WebSocket event socket.
//!
//! Ported from the reference implementation (`Electron V1/src/main/lcu/lcu-client.ts` and
//! `lcu-event-socket.ts`). The LCU serves a self-signed certificate on `127.0.0.1`, so
//! certificate verification is intentionally disabled — but only for this loopback connection;
//! authentication is still required via HTTP Basic and the WebSocket handshake is verified.
//!
//! The WebSocket path uses the project's strict `WebSocketFrameDecoder` rather than a generic
//! client, preserving the reference's malformed-frame handling.

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
    EVENT_SUBSCRIPTION_FRAME, GAMEFLOW_ENDPOINT, LCU_USERNAME, READY_CHECK_ACCEPT_ENDPOINT,
    READY_CHECK_ENDPOINT, SUMMONER_ENDPOINT,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
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
/// connection, which serves a self-signed certificate (the reference uses `rejectUnauthorized:false`).
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

fn tls_config() -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("failed to build TLS protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate { provider }))
        .with_no_client_auth();
    Arc::new(config)
}

fn authorization(credentials: &LcuCredentials) -> String {
    let token = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", LCU_USERNAME, credentials.password));
    format!("Basic {token}")
}

/// Loopback HTTPS REST client for the LCU.
pub struct LcuRestClient {
    credentials: LcuCredentials,
    config: Arc<ClientConfig>,
}

impl LcuRestClient {
    pub fn new(credentials: LcuCredentials) -> Self {
        Self {
            credentials,
            config: tls_config(),
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

    pub async fn get(&self, endpoint: &str) -> Result<serde_json::Value, TransportError> {
        let request =
            LcuRequest::new(HttpMethod::Get, endpoint, None).map_err(TransportError::Request)?;
        self.request(&request).await
    }

    pub async fn request(&self, request: &LcuRequest) -> Result<serde_json::Value, TransportError> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.request_inner(request))
            .await
            .map_err(|_| TransportError::Timeout)?
    }

    async fn request_inner(
        &self,
        request: &LcuRequest,
    ) -> Result<serde_json::Value, TransportError> {
        let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, self.credentials.port))
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;
        let connector = TlsConnector::from(self.config.clone());
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
            authorization(&self.credentials),
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
        if body.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(body).map_err(|error| TransportError::Json(error.to_string()))
    }
}

/// Loopback TLS WebSocket client for LCU events.
pub struct LcuEventSocket {
    credentials: LcuCredentials,
    config: Arc<ClientConfig>,
}

impl LcuEventSocket {
    pub fn new(credentials: LcuCredentials) -> Self {
        Self {
            credentials,
            config: tls_config(),
        }
    }

    pub async fn connect(&self) -> Result<LcuEventConnection, TransportError> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, self.connect_inner())
            .await
            .map_err(|_| TransportError::Timeout)?
    }

    async fn connect_inner(&self) -> Result<LcuEventConnection, TransportError> {
        let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, self.credentials.port))
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;
        tcp.set_nodelay(true)
            .map_err(|error| TransportError::Io(error.to_string()))?;
        let connector = TlsConnector::from(self.config.clone());
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
            authorization(&self.credentials),
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

/// A connected LCU event socket.
pub struct LcuEventConnection {
    tls: tokio_rustls::client::TlsStream<TcpStream>,
    decoder: WebSocketFrameDecoder,
    leftover: Vec<u8>,
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

    /// Returns the next text event, or `Closed` when the connection ends. Ping frames are
    /// answered with a pong and do not surface.
    pub async fn next_event(&mut self) -> Result<SocketEvent, TransportError> {
        loop {
            let chunk = if self.leftover.is_empty() {
                let mut buffer = [0u8; 8192];
                let read = self
                    .tls
                    .read(&mut buffer)
                    .await
                    .map_err(|error| TransportError::Io(error.to_string()))?;
                if read == 0 {
                    return Ok(SocketEvent::Closed);
                }
                buffer[..read].to_vec()
            } else {
                std::mem::take(&mut self.leftover)
            };

            let frames = self
                .decoder
                .push(&chunk)
                .map_err(|error| TransportError::Protocol(error.to_string()))?;
            for frame in frames {
                match frame {
                    DecodedFrame::Text(text) => return Ok(SocketEvent::Text(text)),
                    DecodedFrame::Close(_) => return Ok(SocketEvent::Closed),
                    DecodedFrame::Ping(payload) => self.send_frame(0xA, &payload).await?,
                }
            }
        }
    }
}

fn find_double_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_status(header: &str) -> Option<u16> {
    let line = header.lines().next()?;
    let mut parts = line.split_whitespace();
    parts.next()?; // HTTP/1.1
    parts.next()?.parse::<u16>().ok()
}
