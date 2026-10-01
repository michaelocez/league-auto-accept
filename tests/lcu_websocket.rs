//! LCU WebSocket transport, exercised against a self-signed mock server.
//!
//! Covers the parts that are awkward to test against a live League client:
//!   * TLS to a self-signed loopback server
//!   * HTTP Basic authentication on the WebSocket upgrade
//!   * WebSocket upgrade handshake
//!   * the `[5,"OnJsonApiEvent"]` subscription frame
//!   * receiving and parsing a representative LCU JSON API event
//!
//! The mock mirrors the real LCU's self-signed loopback behaviour. We pin the generated
//! certificate as a trust root, which proves TLS works while keeping verification meaningful.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::RootCertStore;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_hdr_async, connect_async_tls_with_config, Connector};

use league_auto_accept::league as lcu;

const LCU_PASSWORD: &str = "local-secret";

/// Generate a self-signed certificate for `localhost` (what the mock LCU will present).
fn self_signed() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("failed to generate self-signed certificate");
    let cert = certified.cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der()));
    (cert, key)
}

// The WebSocket handshake callback's error type is large by library design; not actionable here.
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn lcu_websocket_tls_auth_subscribe_and_event_roundtrip() {
    // Both `aws-lc-rs` and `ring` are present in the dependency graph, so pick one explicitly.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    let (cert, key) = self_signed();
    let cert_for_client = cert.clone();

    // Mock LCU server: TLS + WebSocket, asserting the Basic auth header, then emitting one event.
    let server_config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("invalid server certificate/key");
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind mock server");
    let port = listener.local_addr().unwrap().port();

    let expected_auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!(
            "{}:{}",
            lcu::LCU_USERNAME,
            LCU_PASSWORD
        ))
    );

    let expected_auth_for_server = expected_auth.clone();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.expect("accept failed");
        let tls = acceptor.accept(tcp).await.expect("TLS accept failed");

        let mut ws = accept_hdr_async(tls, move |request: &Request<()>, response: Response<()>| {
            let auth = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            assert_eq!(
                auth, expected_auth_for_server,
                "unexpected Authorization header"
            );
            assert_eq!(
                request
                    .headers()
                    .get("upgrade")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("websocket"),
                "missing WebSocket upgrade header"
            );
            Ok(response)
        })
        .await
        .expect("WebSocket handshake failed");

        // The client must send the LCU subscription frame first.
        let subscription = ws
            .next()
            .await
            .expect("client closed early")
            .expect("subscription read failed");
        assert_eq!(
            subscription.to_text().expect("subscription was not text"),
            lcu::EVENT_SUBSCRIPTION_FRAME,
            "unexpected subscription frame"
        );

        // Emit a representative `OnJsonApiEvent` event, as the LCU would.
        let event = format!(
            r#"[8,"OnJsonApiEvent",{{"uri":"{}","eventType":"Update","data":"InProgress"}}]"#,
            lcu::GAMEFLOW_ENDPOINT
        );
        ws.send(Message::Text(event)).await.unwrap();

        // Keep the connection open briefly so the client can read.
        tokio::time::sleep(Duration::from_millis(500)).await;
    });

    // Client: rustls with the mock certificate pinned as a trust root.
    let mut roots = RootCertStore::empty();
    roots
        .add(cert_for_client)
        .expect("failed to add trust root");
    let client_config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = Connector::Rustls(Arc::new(client_config));

    let mut request = format!("wss://localhost:{port}/")
        .into_client_request()
        .expect("invalid request");
    request.headers_mut().insert(
        "Authorization",
        expected_auth.parse().expect("invalid header value"),
    );

    let (mut ws, _response) = connect_async_tls_with_config(request, None, false, Some(connector))
        .await
        .expect("client WebSocket connect failed");

    ws.send(Message::Text(lcu::EVENT_SUBSCRIPTION_FRAME.into()))
        .await
        .expect("failed to send subscription");

    let message = ws
        .next()
        .await
        .expect("no event received")
        .expect("event read failed");
    let text = message.into_text().expect("event was not text");
    let parsed: serde_json::Value = serde_json::from_str(text.as_str()).expect("invalid JSON");

    assert_eq!(parsed[0], 8);
    assert_eq!(parsed[1], "OnJsonApiEvent");
    assert_eq!(parsed[2]["uri"], lcu::GAMEFLOW_ENDPOINT);
    assert_eq!(parsed[2]["eventType"], "Update");
    assert_eq!(parsed[2]["data"], "InProgress");

    server.await.expect("server task panicked");
}
