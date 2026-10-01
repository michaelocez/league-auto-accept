//! Deterministic tests for the concrete LCU transports against self-signed mock servers.
//!
//! No live League client is required: a mock LCU presents a self-signed certificate on
//! `127.0.0.1` and asserts the Basic auth header, mirroring the real LCU. Live-League testing
//! remains manual (see the `league-headless` binary).

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::http::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

use league_auto_accept::league::lockfile::LcuCredentials;
use league_auto_accept::league::transport::{LcuEventSocket, LcuRestClient, SocketEvent};

const PASSWORD: &str = "local-secret";

fn credentials(port: u16) -> LcuCredentials {
    LcuCredentials {
        install_path: PathBuf::new(),
        process_name: "LeagueClientUx".into(),
        process_id: 1,
        port,
        password: PASSWORD.into(),
    }
}

fn expected_authorization() -> String {
    let token = base64::engine::general_purpose::STANDARD.encode(format!("riot:{PASSWORD}"));
    format!("Basic {token}")
}

fn server_config() -> (ServerConfig, CertificateDer<'static>) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_der = certified.cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der()));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key)
        .expect("invalid mock certificate");
    (config, cert_der)
}

fn http_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        _ => "Status",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A mock LCU HTTPS endpoint answering the three REST calls the client makes.
async fn spawn_mock_rest(port_holder: tokio::sync::oneshot::Sender<u16>) {
    let (config, _cert) = server_config();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    port_holder
        .send(listener.local_addr().unwrap().port())
        .unwrap();

    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            break;
        };
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let Ok(mut tls) = acceptor.accept(tcp).await else {
                return;
            };
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let Ok(read) = tls.read(&mut chunk).await else {
                    return;
                };
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buffer);
            let request_line = head.lines().next().unwrap_or("");
            assert_eq!(
                head.lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|line| line["authorization:".len()..].trim()),
                Some(expected_authorization().as_str()),
                "mock LCU expected Basic auth"
            );
            let response = if request_line.starts_with("GET /lol-summoner/v1/current-summoner") {
                http_response(200, r#"{"displayName":"Tester"}"#)
            } else if request_line.starts_with("GET /lol-gameflow/v1/gameflow-phase") {
                http_response(200, r#""InProgress""#)
            } else if request_line.starts_with("POST /lol-matchmaking/v1/ready-check/accept") {
                http_response(204, "")
            } else {
                http_response(404, "")
            };
            let _ = tls.write_all(response.as_bytes()).await;
            let _ = tls.flush().await;
        });
    }
}

#[tokio::test]
async fn rest_transport_authenticates_parses_and_accepts() {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(spawn_mock_rest(tx));
    let port = rx.await.unwrap();

    let client = LcuRestClient::new(credentials(port));
    client.verify().await.expect("verify should succeed");
    assert_eq!(client.get_gameflow_phase().await.unwrap(), "InProgress");
    client.accept().await.expect("accept should succeed");
}

#[tokio::test]
async fn rest_transport_reports_non_success_status() {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(spawn_mock_rest(tx));
    let port = rx.await.unwrap();

    let client = LcuRestClient::new(credentials(port));
    // The mock returns 404 for any path other than the fixed endpoints it knows.
    let error = client.get("/lol-nonexistent/v1/thing").await.unwrap_err();
    match error {
        league_auto_accept::league::transport::TransportError::Http { status } => {
            assert_eq!(status, 404);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

// The WebSocket handshake callback's error type is large by library design; not actionable here.
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn event_socket_handshakes_subscribes_and_receives_an_event() {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let (config, _cert) = server_config();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let expected = expected_authorization();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(tcp).await.unwrap();
        let mut ws = accept_hdr_async(tls, move |request: &Request<()>, response: Response<()>| {
            let auth = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            assert_eq!(auth, expected, "event socket must send Basic auth");
            assert_eq!(
                request
                    .headers()
                    .get("upgrade")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("websocket")
            );
            Ok(response)
        })
        .await
        .expect("handshake failed");

        let subscription = ws.next().await.unwrap().unwrap();
        assert_eq!(subscription.to_text().unwrap(), "[5,\"OnJsonApiEvent\"]");

        let event = r#"[8,"OnJsonApiEvent",{"uri":"/lol-gameflow/v1/gameflow-phase","eventType":"Update","data":"InProgress"}]"#;
        ws.send(Message::Text(event.into())).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    });

    let mut connection = LcuEventSocket::new(credentials(port))
        .connect()
        .await
        .expect("event socket should connect");
    match connection
        .next_event()
        .await
        .expect("should receive an event")
    {
        SocketEvent::Text(text) => {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value[2]["uri"], "/lol-gameflow/v1/gameflow-phase");
            assert_eq!(value[2]["data"], "InProgress");
        }
        other => panic!("unexpected socket event: {other:?}"),
    }

    server.await.unwrap();
}
