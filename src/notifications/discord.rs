//! Discord webhook delivery.
//!
//! Strict URL validation, a bounded HTTPS request, no redirects, and an explicit mention
//! allowlist (`allowed_mentions.parse = []`) so text such as `@everyone` cannot mass-ping.

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

use crate::config::settings::{allowed_mention_ids, DiscordMention, MAX_DISCORD_USER_IDS};

const ALLOWED_HOSTS: [&str; 4] = [
    "discord.com",
    "canary.discord.com",
    "ptb.discord.com",
    "discordapp.com",
];
pub const MAX_WEBHOOK_URL_LENGTH: usize = 2048;
/// Discord's own limit for a webhook message, in characters. The preview, the sender and the
/// template cap all derive from this one number.
pub const MAX_CONTENT_LENGTH: usize = 2000;
/// The longest user id Discord issues, so the longest a rendered mention `<@id>` can be is this
/// plus the three characters of `<@` and `>`.
const MAX_DISCORD_USER_ID_LENGTH: usize = 20;
/// How many characters `{mentions}` can expand to: one rendered mention per allowed id, joined by
/// single spaces. Derived, so it cannot drift from the mention cap or the mention format.
pub const MAX_MENTION_EXPANSION_LENGTH: usize =
    MAX_DISCORD_USER_IDS * (MAX_DISCORD_USER_ID_LENGTH + 3) + (MAX_DISCORD_USER_IDS - 1);
/// The longest message template the application accepts and stores, in characters.
///
/// This is the single cap for a saved template, derived from Discord's content limit with the
/// worst-case mention expansion subtracted: every template that fits it also renders inside
/// Discord's limit once its mentions are substituted. The settings store normalises templates with
/// exactly this limit and the editor refuses to go past it, so a template can no longer be accepted
/// at a length the store would silently shorten.
///
/// The cost is that a template with no `{mentions}` keeps headroom it does not strictly need; that
/// is the price of one limit instead of two, and the mention expansion is the only part of the
/// rendered length the template itself cannot control.
///
/// Templates are counted in characters (Unicode scalar values) on both sides, matching the store.
pub const MAX_TEMPLATE_LENGTH: usize = MAX_CONTENT_LENGTH - MAX_MENTION_EXPANSION_LENGTH;
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

/// The Discord webhook URL — the application's only secret.
///
/// The value is readable only through `expose`, and `Debug` is redacted, so it cannot leak through
/// a log line or through derived `Debug` on an enclosing type.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct WebhookSecret(String);

impl WebhookSecret {
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into().trim().to_string())
    }

    pub fn is_configured(&self) -> bool {
        !self.0.is_empty()
    }

    /// Non-secret status: whether a webhook is set, and the (allowlisted) host it points at.
    pub fn status(&self) -> WebhookStatus {
        WebhookStatus {
            configured: self.is_configured(),
            host: parse_discord_webhook_url(&self.0).map(|target| target.host),
        }
    }

    /// The raw URL. Only the notification sender may call this.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookSecret(<redacted>)")
    }
}

/// A secret-free description of the configured webhook, safe to hold in application state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebhookStatus {
    pub configured: bool,
    pub host: Option<String>,
}

impl WebhookStatus {
    /// Short status label for the UI; never contains the secret.
    pub fn label(&self) -> String {
        match (&self.configured, &self.host) {
            (false, _) => "Not configured".to_string(),
            (true, Some(host)) => format!("Configured · {host}"),
            (true, None) => "Configured".to_string(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct WebhookRequest {
    pub url: WebhookSecret,
    pub content: String,
    pub user_ids: Vec<String>,
}

impl std::fmt::Debug for WebhookRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookRequest")
            .field("url", &self.url)
            .field("content", &self.content)
            .field("user_ids", &self.user_ids)
            .finish()
    }
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

/// Validates a Discord webhook URL.
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

/// Renders exactly what a notification would send for `template`, for the local preview.
///
/// Pure and network-free: it applies the same mention allowlist and cap as the sender, so the
/// preview cannot drift from the delivered message.
pub fn preview_notification(template: &str, mentions: &[DiscordMention]) -> String {
    render_notification_message(template, &allowed_mention_ids(mentions))
}

/// Message-length rule shared by the sender and the preview: 1–2,000 characters of *rendered*
/// content, which is the limit Discord itself applies.
pub fn content_error(content: &str) -> Option<&'static str> {
    if content.trim().is_empty() || content.chars().count() > MAX_CONTENT_LENGTH {
        Some("The Discord message must contain 1–2,000 characters.")
    } else {
        None
    }
}

/// The template-length rule, checked on the template as typed rather than on the message it
/// renders to.
///
/// This is the limit the settings store enforces ([`MAX_TEMPLATE_LENGTH`]); the editor uses it so a
/// template that would be shortened on save is refused while it is being written instead. The
/// rendered message is still checked separately by [`content_error`], because a template can sit
/// inside this cap and still be too long once its mentions expand — that combination is what the
/// cap's mention budget is sized to prevent, and checking both keeps it cheap to notice if the two
/// ever drift apart again.
pub fn template_error(template: &str) -> Option<String> {
    let characters = template.chars().count();
    if characters > MAX_TEMPLATE_LENGTH {
        Some(format!(
            "The message template must be at most {MAX_TEMPLATE_LENGTH} characters, so that it still \
             fits {MAX_CONTENT_LENGTH} once mentions are added. This one is {characters} characters \
             and would be shortened when saved."
        ))
    } else {
        None
    }
}

/// Builds the Discord payload with automatic parsing disabled and only explicit IDs allowed.
pub fn discord_webhook_payload(content: &str, user_ids: &[String]) -> String {
    serde_json::json!({
        "content": content,
        "allowed_mentions": { "parse": Vec::<String>::new(), "users": user_ids }
    })
    .to_string()
}

/// What a load of this machine's root certificates produced, counted and judged.
///
/// Built only by [`classify_trust_store`], so "usable" and "empty" are decided in one place: the
/// code that reacts to a store cannot disagree with the code that records it.
#[derive(Debug)]
struct TrustStoreHealth {
    /// Certificates the root store accepted — the only count that decides usability.
    loaded: usize,
    /// Certificates the platform offered and the root store refused as malformed.
    refused: usize,
    /// Errors the platform loader itself reported while reading the store.
    load_errors: usize,
    /// The first loader error's text, kept as a sample of the cause.
    sample_error: Option<String>,
}

impl TrustStoreHealth {
    /// Whether nothing usable came back, so no handshake could succeed.
    fn is_empty(&self) -> bool {
        self.loaded == 0
    }

    /// Whether the store works but something was lost on the way into it.
    fn has_losses(&self) -> bool {
        self.refused > 0 || self.load_errors > 0
    }

    /// The counts in one sentence, with the cause where there is one.
    ///
    /// The empty form is what the user is shown once no webhook can be delivered at all, so it
    /// states what happened rather than describing a retryable network fault; an empty store the
    /// loader did not complain about says that too, because a silently empty store is itself the
    /// diagnostic.
    fn message(&self) -> String {
        let counts = format!(
            "{} certificate(s) loaded, {} refused by the store, {} store load error(s)",
            self.loaded, self.refused, self.load_errors
        );
        if self.is_empty() {
            let cause = match &self.sample_error {
                Some(error) => format!("first error: {error}"),
                None => "the platform loader reported no error".to_string(),
            };
            format!(
                "no usable TLS root certificates on this machine ({counts}; {cause}), so a Discord \
                 webhook cannot be verified"
            )
        } else if self.has_losses() {
            let sample = match &self.sample_error {
                Some(error) => format!("; first error: {error}"),
                None => String::new(),
            };
            format!("TLS root store is incomplete ({counts}{sample})")
        } else {
            format!("TLS root store is usable ({counts})")
        }
    }
}

/// Counts and judges what a native-certificate load produced.
///
/// Pure, and separate from the load itself, so the judgement can be tested without reading this
/// machine's store. `loaded` is what the root store actually holds rather than what it was offered:
/// a certificate the store refused changes nothing about whether a chain can be verified.
fn classify_trust_store(
    loaded: usize,
    refused: usize,
    load_errors: usize,
    sample_error: Option<String>,
) -> TrustStoreHealth {
    TrustStoreHealth {
        loaded,
        refused,
        load_errors,
        sample_error,
    }
}

/// Real HTTPS sender. The network path is not auto-tested (requires internet); URL validation and
/// payload construction are unit-tested below.
pub struct DiscordWebhookClient {
    /// TLS setup is fallible; a failure is reported as a delivery failure, never a panic. An empty
    /// root store is one of those failures, recorded here when the client is built: every handshake
    /// would fail the same way, so it is stated once as the cause instead of looking like a
    /// transport fault on each request.
    config: Result<Arc<ClientConfig>, String>,
}

impl Default for DiscordWebhookClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscordWebhookClient {
    pub fn new() -> Self {
        let native = rustls_native_certs::load_native_certs();
        let mut roots = RootCertStore::empty();
        let mut refused = 0;
        for certificate in native.certs {
            // Counted rather than discarded: a malformed certificate is a fact about this machine's
            // store, and the count is what tells a thin store apart from an untouched one.
            if roots.add(certificate).is_err() {
                refused += 1;
            }
        }
        // The crate's own failures are counted and sampled separately from the store's refusals:
        // they are what explains a store that came back empty.
        let health = classify_trust_store(
            roots.len(),
            refused,
            native.errors.len(),
            native.errors.first().map(|error| error.to_string()),
        );
        Self::from_loaded_roots(roots, health)
    }

    /// Builds the client from an already-loaded root store and that load's classification.
    ///
    /// The seam exists so the empty-store path — the one that cannot be produced on demand — is
    /// reachable from a test without reading this machine's real store or opening a socket.
    fn from_loaded_roots(roots: RootCertStore, health: TrustStoreHealth) -> Self {
        if health.is_empty() {
            // Nothing can be verified with this store, so the cause is recorded where the user can
            // read it (Settings shows error-level records) and carried into every delivery failure.
            let message = health.message();
            log::error!("{message}");
            return Self {
                config: Err(message),
            };
        }
        if health.has_losses() {
            log::warn!("{}", health.message());
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
            .map(Arc::new)
            .map_err(|error| format!("could not initialise Discord TLS: {error}"));
        Self { config }
    }
}

impl DiscordSender for DiscordWebhookClient {
    fn send(&self, request: WebhookRequest) -> WebhookFuture {
        let config = match &self.config {
            Ok(config) => config.clone(),
            Err(error) => {
                let message = format!("Discord notifications unavailable: {error}");
                return Box::pin(async move { WebhookResult::failure(message) });
            }
        };
        Box::pin(async move {
            let Some(target) = parse_discord_webhook_url(request.url.expose()) else {
                return WebhookResult::failure("Enter a valid Discord webhook URL.");
            };
            let content = request.content.trim();
            if let Some(message) = content_error(&request.content) {
                return WebhookResult::failure(message);
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

    /// A root store that genuinely holds one certificate, so a "usable store" fixture is a real
    /// store rather than a classification that merely claims to be one.
    fn usable_roots() -> RootCertStore {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("the test fixture certificate must be generated");
        let mut roots = RootCertStore::empty();
        roots
            .add(certified.cert.der().clone())
            .expect("a generated certificate must be usable as a trust anchor");
        roots
    }

    /// A client whose store loaded a certificate, built through the same path the real one uses, so
    /// no test here depends on this machine's OS store.
    fn client_with_usable_store() -> DiscordWebhookClient {
        let roots = usable_roots();
        let loaded = roots.len();
        DiscordWebhookClient::from_loaded_roots(roots, classify_trust_store(loaded, 0, 0, None))
    }

    /// Sends one request through a client and answers what the sender made of it.
    async fn refusal(
        client: &DiscordWebhookClient,
        url: String,
        content: &str,
        user_ids: Vec<String>,
    ) -> WebhookResult {
        client
            .send(WebhookRequest {
                url: WebhookSecret::new(url),
                content: content.into(),
                user_ids,
            })
            .await
    }

    /// Whether a classification message names all three counts: a message that named only one of
    /// them would still read plausibly, which is exactly what must not happen.
    fn names_counts(message: &str, loaded: usize, refused: usize, load_errors: usize) -> bool {
        message.contains(&format!("{loaded} certificate(s) loaded"))
            && message.contains(&format!("{refused} refused by the store"))
            && message.contains(&format!("{load_errors} store load error(s)"))
    }

    /// A store that loaded everything is usable, has nothing to report, and says the counts.
    #[test]
    fn a_trust_store_that_loaded_everything_is_usable_and_names_its_counts() {
        let health = classify_trust_store(120, 0, 0, None);
        assert!(!health.is_empty(), "120 certificates is not an empty store");
        assert!(
            !health.has_losses(),
            "nothing was dropped and nothing failed"
        );

        let message = health.message();
        assert!(names_counts(&message, 120, 0, 0), "{message}");
        assert!(message.contains("usable"), "{message}");
    }

    /// A usable store that lost something is still usable — and the sentence says what was lost,
    /// including the loader's own error, rather than reporting a clean load.
    #[test]
    fn a_usable_trust_store_with_losses_reports_them_without_becoming_unusable() {
        let health = classify_trust_store(
            118,
            2,
            1,
            Some("failed to read PEM from file: Access is denied".to_string()),
        );
        assert!(!health.is_empty(), "the store still holds certificates");
        assert!(
            health.has_losses(),
            "two refusals and a load error are losses"
        );

        let message = health.message();
        assert!(names_counts(&message, 118, 2, 1), "{message}");
        assert!(message.contains("incomplete"), "{message}");
        assert!(
            message.contains("failed to read PEM from file: Access is denied"),
            "the sample error must survive into the record: {message}"
        );

        // Losses with no reported loader error still count as losses, and then there is no sample
        // to give: the sentence must not invent one.
        let refused_only = classify_trust_store(118, 2, 0, None);
        assert!(!refused_only.is_empty());
        assert!(refused_only.has_losses());
        let refused_message = refused_only.message();
        assert!(
            names_counts(&refused_message, 118, 2, 0),
            "{refused_message}"
        );
        assert!(
            !refused_message.contains("first error"),
            "there is no loader error to sample: {refused_message}"
        );
    }

    /// An empty store is empty, and the message says so as the cause — naming the loader errors that
    /// produced it, because that text is what the user is shown once nothing can be delivered.
    #[test]
    fn an_empty_trust_store_names_the_loader_error_that_caused_it() {
        let health = classify_trust_store(
            0,
            0,
            2,
            Some("opening the root store: Access is denied".to_string()),
        );
        assert!(health.is_empty(), "no certificate was loaded");
        assert!(
            health.has_losses(),
            "the load errors are what explain the empty store"
        );

        let message = health.message();
        assert!(names_counts(&message, 0, 0, 2), "{message}");
        assert!(
            message.contains("no usable TLS root certificates"),
            "{message}"
        );
        assert!(
            message.contains("opening the root store: Access is denied"),
            "the cause must be the cause, not a restatement: {message}"
        );
    }

    /// The same emptiness without a loader error is still empty, and says that the loader was
    /// silent: a store that returns nothing and complains about nothing is a different fact from a
    /// store that failed loudly, and both have to be legible.
    #[test]
    fn an_empty_trust_store_without_load_errors_says_the_loader_reported_nothing() {
        for health in [
            classify_trust_store(0, 0, 0, None),
            // Every certificate that was offered was malformed: still an empty store, still no
            // loader error.
            classify_trust_store(0, 5, 0, None),
        ] {
            assert!(health.is_empty(), "nothing usable was loaded");
            let message = health.message();
            assert!(
                message.contains("no usable TLS root certificates"),
                "{message}"
            );
            assert!(
                message.contains("the platform loader reported no error"),
                "{message}"
            );
            assert!(!message.contains("first error"), "{message}");
            assert!(
                names_counts(&message, 0, health.refused, 0),
                "the counts are still named: {message}"
            );
        }
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
    fn webhook_secret_is_redacted_and_reports_non_secret_status() {
        let id = "1".repeat(17);
        let secret =
            WebhookSecret::new(format!("https://discord.com/api/webhooks/{id}/{}", token()));
        assert!(secret.is_configured());
        assert_eq!(secret.status().host.as_deref(), Some("discord.com"));
        assert_eq!(secret.status().label(), "Configured · discord.com");

        let debugged = format!("{secret:?}");
        assert_eq!(debugged, "WebhookSecret(<redacted>)");
        assert!(!debugged.contains("discord.com"));

        let request = WebhookRequest {
            url: secret,
            content: "test".into(),
            user_ids: Vec::new(),
        };
        let rendered = format!("{request:?}");
        assert!(!rendered.contains("discord.com"), "{rendered}");

        let empty = WebhookSecret::new("");
        assert!(!empty.is_configured());
        assert_eq!(empty.status().label(), "Not configured");
    }

    /// No delivery failure may echo the webhook URL: its path *is* the token, and these messages are
    /// exactly what the in-memory diagnostics report can now retain.
    ///
    /// Every refusal reachable without a socket is driven here through the real sender. The socket
    /// failures take the same shape — `post` discards the transport error entirely and answers with
    /// one of its own fixed literals — so the URL cannot reach a message from any of them either.
    #[tokio::test]
    async fn no_delivery_failure_echoes_the_webhook_url() {
        let id = "12345678901234567";
        let token = format!("sentinel{}", "t".repeat(32));
        // A fixture store rather than this machine's: what is being tested is the sender, and the
        // test must not depend on which certificates are installed here.
        let client = client_with_usable_store();

        let sentinel = format!("https://discord.com/api/webhooks/{id}/{token}");
        // Each of these is refused before a socket is opened, so the test stays offline: the URL
        // itself, and then two request faults behind a *valid* URL.
        let refusals = [
            (
                format!("https://evil.test/api/webhooks/{id}/{token}"),
                "hello",
                Vec::new(),
            ),
            (sentinel.clone(), "", Vec::new()),
            (sentinel, "hello", vec!["not-an-id".to_string()]),
        ];
        for (url, content, user_ids) in refusals {
            let result = client
                .send(WebhookRequest {
                    url: WebhookSecret::new(url),
                    content: content.into(),
                    user_ids,
                })
                .await;
            assert!(!result.ok, "this request is meant to be refused");
            assert!(!result.message.contains(&token), "{}", result.message);
            assert!(!result.message.contains(id), "{}", result.message);
        }
    }

    /// The defect this guards: with an empty root store the client was built anyway, so every
    /// request failed at the handshake and the user was told only that the webhook request failed —
    /// indistinguishable from Discord being unreachable, and nothing anywhere said the machine had
    /// no usable certificates.
    ///
    /// The request below is valid in every other way and is never sent: the store's own cause is
    /// what the refusal has to name.
    #[tokio::test]
    async fn a_client_with_an_empty_trust_store_names_the_cause_instead_of_a_network_fault() {
        let client = DiscordWebhookClient::from_loaded_roots(
            RootCertStore::empty(),
            classify_trust_store(
                0,
                0,
                1,
                Some("opening the root store: Access is denied".to_string()),
            ),
        );

        let result = refusal(
            &client,
            format!(
                "https://discord.com/api/webhooks/{}/{}",
                "1".repeat(17),
                token()
            ),
            "hello",
            Vec::new(),
        )
        .await;

        assert!(!result.ok, "an empty store must refuse the request");
        assert!(
            result
                .message
                .starts_with("Discord notifications unavailable: "),
            "the refusal must keep the configured-failure shape: {}",
            result.message
        );
        assert!(
            result.message.contains("no usable TLS root certificates"),
            "the refusal must name the real cause: {}",
            result.message
        );
        assert!(
            result
                .message
                .contains("opening the root store: Access is denied"),
            "the refusal must carry the loader's own error: {}",
            result.message
        );
        assert!(
            !result.message.contains("Discord webhook request failed."),
            "an empty store must not read as a network fault: {}",
            result.message
        );
    }

    /// A store that loaded certificates must behave exactly as it did before the store was judged:
    /// each pre-socket refusal keeps its own wording. Every case here is refused before a socket is
    /// opened, so the test stays offline — and each would instead read "Discord notifications
    /// unavailable: …" if the usable store had been turned into a failure.
    #[tokio::test]
    async fn a_usable_trust_store_leaves_the_existing_request_validation_unchanged() {
        let client = client_with_usable_store();
        let id = "1".repeat(17);
        let valid_url = format!("https://discord.com/api/webhooks/{id}/{}", token());

        let invalid_host = refusal(
            &client,
            format!("https://evil.test/api/webhooks/{id}/{}", token()),
            "hello",
            Vec::new(),
        )
        .await;
        assert_eq!(invalid_host.message, "Enter a valid Discord webhook URL.");

        let empty_content = refusal(&client, valid_url.clone(), "", Vec::new()).await;
        assert_eq!(
            empty_content.message,
            "The Discord message must contain 1–2,000 characters."
        );

        let invalid_id = refusal(&client, valid_url, "hello", vec!["not-an-id".to_string()]).await;
        assert_eq!(
            invalid_id.message,
            "One or more Discord user IDs are invalid."
        );
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

    #[test]
    fn preview_renders_the_sent_message_with_the_mention_cap_applied() {
        let mentions: Vec<DiscordMention> = (0..7)
            .map(|index| DiscordMention {
                id: format!("{:017}", index + 1),
                nickname: String::new(),
            })
            .collect();

        let preview = preview_notification("{mentions} your queue has popped.", &mentions);
        assert_eq!(
            preview.matches("<@").count(),
            crate::config::settings::MAX_DISCORD_USER_IDS
        );
        assert!(preview.ends_with("your queue has popped."));
        assert_eq!(
            preview_notification("no placeholder", &mentions),
            "no placeholder"
        );
    }

    #[test]
    fn content_error_flags_only_empty_and_oversized_messages() {
        assert!(content_error("").is_some());
        assert!(content_error("   ").is_some());
        assert!(content_error("hello").is_none());
        assert!(content_error(&"x".repeat(MAX_CONTENT_LENGTH)).is_none());
        assert!(content_error(&"x".repeat(MAX_CONTENT_LENGTH + 1)).is_some());
    }

    /// The defect this guards: a template between the editor's rendered-message limit (2,000) and
    /// the store's template cap (1,800) passed validation, then came back shorter than it was typed.
    ///
    /// The cap is now the store's cap, so there is exactly one length a template can be accepted at.
    /// Without the fix the `MAX_TEMPLATE_LENGTH + 1` assertion fails, because the template is still
    /// inside Discord's 2,000 and the editor reports nothing.
    #[test]
    fn a_template_is_accepted_exactly_up_to_the_length_the_store_keeps() {
        let at_cap = "t".repeat(MAX_TEMPLATE_LENGTH);
        assert_eq!(
            template_error(&at_cap),
            None,
            "a template the store keeps intact must be accepted"
        );

        let one_over = "t".repeat(MAX_TEMPLATE_LENGTH + 1);
        let message =
            template_error(&one_over).expect("one character over the cap must be refused");
        assert!(
            message.contains(&MAX_TEMPLATE_LENGTH.to_string()),
            "the refusal must say what the limit is: {message}"
        );
        assert!(
            message.contains("shortened"),
            "the refusal must say what would otherwise happen: {message}"
        );

        // The old limit is squarely inside the gap this closes, so pin the relationship rather than
        // a magic number: anything the editor accepts has to fit the store.
        assert!(at_cap.chars().count() <= MAX_TEMPLATE_LENGTH);
    }

    /// The mention budget is what makes one limit serve both roles: a template at the cap, expanded
    /// with the maximum number of mentions, lands exactly on Discord's content limit and no further.
    ///
    /// If the budget were too small, a stored template could render over Discord's limit and the
    /// sender would refuse it; too large, and the editor would refuse templates the store keeps fine.
    #[test]
    fn the_worst_case_render_of_a_template_at_the_cap_fits_discord() {
        let ids: Vec<String> = (0..crate::config::settings::MAX_DISCORD_USER_IDS)
            .map(|index| format!("{:020}", index + 1))
            .collect();
        let template =
            "{mentions}".to_string() + &"t".repeat(MAX_TEMPLATE_LENGTH - "{mentions}".len());
        assert_eq!(
            template.chars().count(),
            MAX_TEMPLATE_LENGTH,
            "the fixture itself must sit exactly on the cap"
        );

        let rendered = render_notification_message(&template, &ids);
        // The budget is a ceiling, not an exact fit: the longest *possible* mention is 23 characters
        // while these ids are 20 digits, so the render lands just inside Discord's limit with the
        // difference as spare headroom. What matters is that it never exceeds it, and that the
        // budget is what leaves the room.
        assert!(
            rendered.chars().count() <= MAX_CONTENT_LENGTH,
            "a template at the cap must render inside Discord's limit, got {}",
            rendered.chars().count()
        );
        assert!(
            content_error(&rendered).is_none(),
            "a template at the cap must render inside Discord's limit"
        );
        assert!(
            rendered.chars().count() >= MAX_CONTENT_LENGTH - MAX_MENTION_EXPANSION_LENGTH,
            "the template cap must actually use its budget rather than stay far below it, got {}",
            rendered.chars().count()
        );

        // One more character anywhere, and both rules agree it is too much.
        let over = format!("{template}t");
        assert!(template_error(&over).is_some());
    }
}
