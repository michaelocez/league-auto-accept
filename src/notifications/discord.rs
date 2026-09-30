//! Discord webhook delivery.
//!
//! Ported from the reference implementation (`Electron V1/src/main/discord-webhook-client.ts`).
//! Strict URL validation, bounded HTTPS request, no redirects, and an explicit mention allowlist
//! (`allowed_mentions.parse = []`) so text such as `@everyone` cannot mass-ping.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

const ALLOWED_HOSTS: [&str; 4] = [
    "discord.com",
    "canary.discord.com",
    "ptb.discord.com",
    "discordapp.com",
];
pub const MAX_WEBHOOK_URL_LENGTH: usize = 2048;
pub const MAX_CONTENT_LENGTH: usize = 2000;
const MAX_RESPONSE_BYTES: usize = 8 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(7);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebhookResult {
    pub ok: bool,
    pub message: String,
}

impl WebhookResult {
    pub fn ok() -> Self {
        Self {
            ok: true,
            message: "Discord webhook sent.".into(),
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebhookRequest {
    pub url: String,
    pub content: String,
    pub user_ids: Vec<String>,
}

pub type WebhookFuture = Pin<Box<dyn Future<Output = WebhookResult> + Send + 'static>>;

/// Injectable sender so notification logic is testable without the network.
pub trait DiscordSender: Send + Sync {
    fn send(&self, request: WebhookRequest) -> WebhookFuture;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebhookTarget {
    pub host: String,
    pub path: String,
}

/// Validates a Discord webhook URL. Mirrors `parseDiscordWebhookUrl`.
pub fn parse_discord_webhook_url(value: &str) -> Option<WebhookTarget> {
    let value = value.trim();
    if value.len() > MAX_WEBHOOK_URL_LENGTH {
        return None;
    }
    let rest = value.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    if host.is_empty() || host.contains('@') || host.contains(':') {
        return None; // credentials or explicit port
    }
    let host = host.to_ascii_lowercase();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return None;
    }
    let path = format!("/{path}");
    if path.contains('?') || path.contains('#') {
        return None;
    }
    if !is_valid_webhook_path(&path) {
        return None;
    }
    Some(WebhookTarget { host, path })
}

fn is_valid_webhook_path(path: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    let parts: Vec<&str> = trimmed.split('/').collect();
    // ["", "api", ("vN")?, "webhooks", "<id>", "<token>"]
    if parts.first() != Some(&"") || parts.get(1) != Some(&"api") {
        return false;
    }
    let webhooks_index = match parts.get(2) {
        Some(&"webhooks") => 2,
        Some(version) if is_version_segment(version) => 3,
        _ => return false,
    };
    if parts.len() != webhooks_index + 3 || parts.get(webhooks_index) != Some(&"webhooks") {
        return false;
    }
    let id = parts[webhooks_index + 1];
    let token = parts[webhooks_index + 2];
    let id_ok = (17..=20).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit());
    let token_ok = (20..=200).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    id_ok && token_ok
}

fn is_version_segment(value: &str) -> bool {
    value
        .strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Renders `{mentions}` as `<@id>` for every configured mention; all occurrences are replaced.
pub fn render_notification_message(template: &str, user_ids: &[String]) -> String {
    let mentions = user_ids
        .iter()
        .map(|id| format!("<@{id}>"))
        .collect::<Vec<_>>()
        .join(" ");
    template.replace("{mentions}", &mentions).trim().to_string()
}

/// Builds the Discord payload with automatic parsing disabled and only explicit IDs allowed.
pub fn discord_webhook_payload(content: &str, user_ids: &[String]) -> String {
    serde_json::json!({
        "content": content,
        "allowed_mentions": { "parse": Vec::<String>::new(), "users": user_ids }
    })
    .to_string()
}

/// Real HTTPS sender. The network path is not auto-tested (requires internet); URL validation and
/// payload construction are unit-tested below.
pub struct DiscordWebhookClient {
    config: Arc<ClientConfig>,
}

impl Default for DiscordWebhookClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscordWebhookClient {
    pub fn new() -> Self {
        let mut roots = RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        for certificate in native.certs {
            let _ = roots.add(certificate);
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("failed to build TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Self {
            config: Arc::new(config),
        }
    }
}

impl DiscordSender for DiscordWebhookClient {
    fn send(&self, request: WebhookRequest) -> WebhookFuture {
        let config = self.config.clone();
        Box::pin(async move {
            let Some(target) = parse_discord_webhook_url(&request.url) else {
                return WebhookResult::failure("Enter a valid Discord webhook URL.");
            };
            let content = request.content.trim();
            if content.is_empty() || content.chars().count() > MAX_CONTENT_LENGTH {
                return WebhookResult::failure(
                    "The Discord message must contain 1–2,000 characters.",
                );
            }
            if !request
                .user_ids
                .iter()
                .all(|id| (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()))
            {
                return WebhookResult::failure("One or more Discord user IDs are invalid.");
            }
            let body = discord_webhook_payload(content, &request.user_ids);
            match tokio::time::timeout(REQUEST_TIMEOUT, post(&config, &target, &body)).await {
                Ok(result) => result,
                Err(_) => WebhookResult::failure("Discord webhook request timed out."),
            }
        })
    }
}

async fn post(config: &Arc<ClientConfig>, target: &WebhookTarget, body: &str) -> WebhookResult {
    let tcp = match TcpStream::connect((target.host.as_str(), 443u16)).await {
        Ok(stream) => stream,
        Err(_) => return WebhookResult::failure("Discord webhook request failed."),
    };
    let server_name = match ServerName::try_from(target.host.clone()) {
        Ok(name) => name,
        Err(_) => return WebhookResult::failure("Discord webhook request failed."),
    };
    let connector = TlsConnector::from(config.clone());
    let mut tls = match connector.connect(server_name, tcp).await {
        Ok(stream) => stream,
        Err(_) => return WebhookResult::failure("Discord webhook request failed."),
    };

    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nUser-Agent: League-Auto-Accept\r\nConnection: close\r\n\r\n{}",
        target.path,
        target.host,
        body.len(),
        body
    );
    if tls.write_all(request.as_bytes()).await.is_err() {
        return WebhookResult::failure("Discord webhook request failed.");
    }
    if tls.flush().await.is_err() {
        return WebhookResult::failure("Discord webhook request failed.");
    }

    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match tls.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&chunk[..read]);
                if response.len() > MAX_RESPONSE_BYTES {
                    return WebhookResult::failure("Discord webhook response was too large.");
                }
            }
            Err(_) => {
                if response.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
                return WebhookResult::failure("Discord webhook request failed.");
            }
        }
    }

    let head = String::from_utf8_lossy(&response);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    if (200..300).contains(&status) {
        WebhookResult::ok()
    } else {
        WebhookResult::failure(format!("Discord returned HTTP {status}."))
    }
}

/// Convenience for tests: build the `{content, allowed_mentions}` JSON as a `Value`.
pub fn payload_value(content: &str, user_ids: &[String]) -> Value {
    serde_json::from_str(&discord_webhook_payload(content, user_ids)).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> String {
        "t".repeat(40)
    }

    #[test]
    fn allows_only_known_discord_https_webhook_urls() {
        let id = "1".repeat(17);
        assert_eq!(
            parse_discord_webhook_url(&format!(
                "https://discord.com/api/webhooks/{id}/{}",
                token()
            ))
            .map(|t| t.host),
            Some("discord.com".into())
        );
        assert_eq!(
            parse_discord_webhook_url(&format!(
                "https://canary.discord.com/api/v10/webhooks/{id}/{}",
                token()
            ))
            .map(|t| t.host),
            Some("canary.discord.com".into())
        );
    }

    #[test]
    fn rejects_lookalike_hosts_credentials_ports_queries_and_malformed_paths() {
        let id = "1".repeat(17);
        let invalid = [
            format!("http://discord.com/api/webhooks/{id}/{}", token()),
            format!(
                "https://discord.com.evil.test/api/webhooks/{id}/{}",
                token()
            ),
            format!(
                "https://user:pass@discord.com/api/webhooks/{id}/{}",
                token()
            ),
            format!("https://discord.com:444/api/webhooks/{id}/{}", token()),
            format!(
                "https://discord.com/api/webhooks/{id}/{}?wait=true",
                token()
            ),
            "https://discord.com/api/webhooks/not-an-id/token".into(),
        ];
        for value in invalid {
            assert!(
                parse_discord_webhook_url(&value).is_none(),
                "{value:?} should be rejected"
            );
        }
    }

    #[test]
    fn allows_mentions_only_for_explicit_ids() {
        let payload = payload_value(
            "@everyone <@12345678901234567>",
            &["12345678901234567".to_string()],
        );
        assert_eq!(
            payload["allowed_mentions"],
            serde_json::json!({ "parse": [], "users": ["12345678901234567"] })
        );
    }

    #[test]
    fn renders_all_mentions_everywhere() {
        let ids = vec![
            "12345678901234567".to_string(),
            "23456789012345678".to_string(),
        ];
        assert_eq!(
            render_notification_message("{mentions} accepted for {mentions}", &ids),
            "<@12345678901234567> <@23456789012345678> accepted for <@12345678901234567> <@23456789012345678>"
        );
        assert_eq!(
            render_notification_message("no placeholder", &ids),
            "no placeholder"
        );
    }
}
