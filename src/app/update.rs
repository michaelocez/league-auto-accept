//! The application's shipped version, and the quiet release check.
//!
//! The version comes from Cargo's compile-time metadata, so the line Settings shows is the number
//! the binary was built with rather than a string that can drift from `Cargo.toml`.
//!
//! The check runs once per session, only when Settings is opened, and makes one HTTPS `GET` of a
//! fixed endpoint. The answer decides whether one version string is newer than another, and then
//! offers a link. Nothing is downloaded, executed or persisted, and every failure is simply "no
//! answer" — never a message, never a retry, never a panic.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

/// The version this build reports. Compile-time, so it cannot disagree with `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The project's releases page — the fixed destination the version line opens.
///
/// When the check finds a newer release it offers that release's own link instead, but only after
/// [`release_target`] has passed it; this page is the fallback when it does not.
pub const RELEASES_PAGE: &str = "https://github.com/michaelocez/league-auto-accept/releases";

/// The one host and path the check talks to. Both are constants, so no request-building path can
/// take a caller-supplied URL, and there are no query parameters.
const LATEST_RELEASE_HOST: &str = "api.github.com";
const LATEST_RELEASE_PATH: &str = "/repos/michaelocez/league-auto-accept/releases/latest";

/// How the check identifies itself: the app's name and version, and nothing about the user, the
/// machine or the installation.
const USER_AGENT: &str = concat!("League-Auto-Accept/", env!("CARGO_PKG_VERSION"));

/// At most this much of the answer is read — which therefore bounds the body too, since the head
/// shares the buffer. A real `releases/latest` body is a few kilobytes.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// Bounds the whole request, so a connect or a read that never completes cannot stall the check.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(7);

/// The only prefix a link from the network may have if it is to be opened.
const ALLOWED_RELEASE_URL_PREFIX: &str = "https://github.com/michaelocez/league-auto-accept/";

/// Why a lookup produced no body: which stage gave up.
///
/// Opaque to callers, which only ever need "no answer" — and it is recorded at `debug` and nowhere
/// else, so it cannot become a message the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateError {
    /// TLS could not be built, or the handshake was refused.
    Tls,
    /// The connection could not be opened, or the request could not be sent.
    Connect,
    /// Nothing arrived inside [`REQUEST_TIMEOUT`].
    Timeout,
    /// The status was anything but 200 — rate limiting included.
    Status(u16),
    /// The answer grew past [`MAX_BODY_BYTES`].
    TooLarge,
    /// The answer could not be read, or never produced a complete head.
    Read,
}

/// A newer release worth telling the user about, and the page to open for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNotice {
    /// The release's tag, exactly as the API named it, so it may carry a leading `v`.
    pub version: String,
    /// The release's own page when it is on this project's GitHub path, and the fixed releases page
    /// otherwise.
    pub url: String,
}

/// The once-per-session gate for the check.
///
/// Opening Settings is what asks for the check, so without a gate every visit would ask again. The
/// gate is state rather than a condition on the send, so a check that could not even be queued is
/// not retried either.
#[derive(Debug, Default)]
pub struct CheckGate {
    started: bool,
}

impl CheckGate {
    /// Marks the session's check as started and reports whether this call is the one that started
    /// it: `true` exactly once, and `false` for every later call.
    pub fn begin(&mut self) -> bool {
        let first = !self.started;
        self.started = true;
        first
    }
}

/// The body a check reads, as one future.
pub type FetchFuture = Pin<Box<dyn Future<Output = Result<String, UpdateError>> + Send + 'static>>;

/// The seam the check reads its body through.
///
/// Production is [`GithubReleaseFetcher`]; tests inject a fake, so the parsing, the version
/// comparison and the URL guard are all exercised without a socket.
pub trait ReleaseFetcher: Send + Sync {
    fn fetch(&self) -> FetchFuture;
}

/// The real fetch: one bounded HTTPS `GET` of the fixed endpoint.
pub struct GithubReleaseFetcher {
    /// TLS setup is fallible; a failure is "no answer", never a panic.
    config: Result<Arc<ClientConfig>, UpdateError>,
}

impl Default for GithubReleaseFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl GithubReleaseFetcher {
    pub fn new() -> Self {
        // Mirrors `DiscordWebhookClient::new`: the OS root store, the ring provider and the safe
        // default protocol versions. Deliberately duplicated rather than shared, so this feature
        // does not touch the released Discord request path and each client keeps its own outcome.
        let mut roots = RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        for certificate in native.certs {
            let _ = roots.add(certificate);
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
            .map(Arc::new)
            .map_err(|_| UpdateError::Tls);
        Self { config }
    }
}

impl ReleaseFetcher for GithubReleaseFetcher {
    fn fetch(&self) -> FetchFuture {
        let config = match &self.config {
            Ok(config) => config.clone(),
            Err(error) => {
                let error = *error;
                return Box::pin(async move { Err(error) });
            }
        };
        Box::pin(async move {
            match tokio::time::timeout(REQUEST_TIMEOUT, get(&config)).await {
                Ok(result) => result,
                Err(_) => Err(UpdateError::Timeout),
            }
        })
    }
}

/// One bounded HTTPS `GET` of the fixed endpoint.
///
/// The read loop is the Discord sender's, including its treatment of a socket that closes after the
/// headers: once the head has arrived, a read error ends the body rather than failing the request.
async fn get(config: &Arc<ClientConfig>) -> Result<String, UpdateError> {
    let tcp = TcpStream::connect((LATEST_RELEASE_HOST, 443u16))
        .await
        .map_err(|_| UpdateError::Connect)?;
    let server_name =
        ServerName::try_from(LATEST_RELEASE_HOST.to_string()).map_err(|_| UpdateError::Tls)?;
    let mut tls = TlsConnector::from(config.clone())
        .connect(server_name, tcp)
        .await
        .map_err(|_| UpdateError::Tls)?;

    let request = format!(
        "GET {LATEST_RELEASE_PATH} HTTP/1.1\r\nHost: {LATEST_RELEASE_HOST}\r\n\
         Accept: application/vnd.github+json\r\nUser-Agent: {USER_AGENT}\r\nConnection: close\r\n\r\n"
    );
    tls.write_all(request.as_bytes())
        .await
        .map_err(|_| UpdateError::Connect)?;
    tls.flush().await.map_err(|_| UpdateError::Connect)?;

    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match tls.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&chunk[..read]);
                if response.len() > MAX_BODY_BYTES {
                    return Err(UpdateError::TooLarge);
                }
            }
            Err(_) if response.windows(4).any(|window| window == b"\r\n\r\n") => break,
            Err(_) => return Err(UpdateError::Read),
        }
    }

    let response = String::from_utf8_lossy(&response);
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    if status != 200 {
        // Rate limiting arrives as 403 or 429 like any other non-200 status.
        return Err(UpdateError::Status(status));
    }
    Ok(response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default())
}

/// Runs the session's single release check and answers with a newer release, if there is one.
///
/// Every failure — a refused TLS handshake, an unreachable host, a timeout, a non-200 answer, a
/// rate limit, a truncated or unparsable body, a body over the cap — is the same "no answer": the
/// caller shows the bare version line and this leaves at most a `debug!` record. Nothing retries.
pub async fn check_release(fetcher: &dyn ReleaseFetcher) -> Option<ReleaseNotice> {
    let body = match fetcher.fetch().await {
        // The cap is applied here as well as inside the transport, so an injected fetcher cannot
        // hand the parser more than the real transport would have allowed.
        Ok(body) if body.len() <= MAX_BODY_BYTES => body,
        Ok(body) => {
            log::debug!(
                "release check: the answer was {} bytes, over the {MAX_BODY_BYTES}-byte cap",
                body.len()
            );
            return None;
        }
        Err(error) => {
            log::debug!("release check: no answer ({error:?})");
            return None;
        }
    };

    let Some((tag_name, html_url)) = parse_release(&body) else {
        log::debug!("release check: the answer did not name a release");
        return None;
    };
    if !is_newer(VERSION, &tag_name) {
        log::debug!("release check: {tag_name} is not newer than {VERSION}");
        return None;
    }

    let notice = ReleaseNotice {
        version: tag_name,
        url: release_target(&html_url).to_string(),
    };
    log::debug!("release check: version {} is available", notice.version);
    Some(notice)
}

/// Reads the two fields the check wants out of GitHub's release JSON, as `(tag_name, html_url)`.
///
/// Both have to be non-empty strings; anything else — a missing field, the wrong type, a body that
/// is not JSON at all — is "no answer". No other field is read or kept, so a response cannot
/// influence anything beyond which version and which link are considered.
fn parse_release(body: &str) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let tag_name = value.get("tag_name")?.as_str()?.trim();
    let html_url = value.get("html_url")?.as_str()?.trim();
    if tag_name.is_empty() || html_url.is_empty() {
        return None;
    }
    Some((tag_name.to_string(), html_url.to_string()))
}

/// Whether `latest` is a strictly newer release than `current`.
///
/// Only a clean three-part numeric version (with an optional leading `v`) is comparable. Anything
/// else — empty, `1.4`, `latest`, `1.5.0-rc1`, `1.4.1.0` — returns `false`, because claiming an
/// update on a version that cannot be parsed is worse than staying quiet. Equal is not newer.
///
/// Each segment is compared as a number, so `1.10.0` is newer than `1.9.9` rather than older.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

/// A three-part numeric version, or nothing.
///
/// A leading `v` (GitHub tags carry one) is accepted; everything else must be digits — no
/// pre-release suffix, no build metadata. A segment too large for a `u64` is not comparable.
fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.trim();
    let digits = value.strip_prefix(['v', 'V']).unwrap_or(value);
    let mut parts = digits.split('.');
    let major = parse_segment(parts.next()?)?;
    let minor = parse_segment(parts.next()?)?;
    let patch = parse_segment(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn parse_segment(segment: &str) -> Option<u64> {
    if segment.is_empty() || !segment.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    segment.parse::<u64>().ok()
}

/// The URL to open for a release: its own page only when that page is on this project's own GitHub
/// path, and the fixed releases page otherwise.
///
/// The link came over the network, so it is not followed blindly — a response that named any other
/// host would otherwise be able to send the browser wherever it liked.
pub fn release_target(html_url: &str) -> &str {
    if html_url.starts_with(ALLOWED_RELEASE_URL_PREFIX) {
        html_url
    } else {
        RELEASES_PAGE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic answer: the two fields the check reads, plus fields it must ignore.
    const NEWER_BODY: &str = r#"{"tag_name":"v9.9.9","html_url":"https://github.com/michaelocez/league-auto-accept/releases/tag/v9.9.9","name":"ignored","body":"ignored","assets":[{"browser_download_url":"https://evil.test/x.exe"}]}"#;

    /// A fetcher that answers with whatever the test chose, so nothing here opens a socket.
    struct FakeFetcher {
        result: Result<String, UpdateError>,
    }

    impl FakeFetcher {
        fn answering(body: impl Into<String>) -> Self {
            Self {
                result: Ok(body.into()),
            }
        }

        fn failing(error: UpdateError) -> Self {
            Self { result: Err(error) }
        }
    }

    impl ReleaseFetcher for FakeFetcher {
        fn fetch(&self) -> FetchFuture {
            let result = self.result.clone();
            Box::pin(async move { result })
        }
    }

    /// The line is only worth showing if the number in it is a real version, and the link is only
    /// safe to open if it is HTTPS on this project's own repositories.
    #[test]
    fn the_shown_version_is_a_clean_three_part_number_and_the_link_is_this_project() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "{VERSION} must have exactly three parts");
        for part in parts {
            assert!(
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()),
                "{VERSION} must be numeric throughout"
            );
        }

        assert!(
            RELEASES_PAGE.starts_with(ALLOWED_RELEASE_URL_PREFIX),
            "{RELEASES_PAGE} must be the project's own HTTPS page"
        );
    }

    #[test]
    fn a_newer_release_is_newer_while_an_equal_or_older_one_is_not() {
        assert!(is_newer("1.4.1", "1.4.2"));
        assert!(is_newer("1.4.1", "1.5.0"));
        assert!(is_newer("1.4.1", "2.0.0"));

        assert!(!is_newer("1.4.1", "1.4.1"), "equal is not newer");
        assert!(!is_newer("1.4.1", "1.4.0"));
        assert!(!is_newer("1.4.1", "1.3.9"));
        assert!(!is_newer("1.4.1", "0.9.0"));
    }

    /// GitHub tags carry a `v`, and the segments are numbers rather than text: without the numeric
    /// comparison the lexicographic answer for `1.10.0` against `1.9.9` would be backwards.
    #[test]
    fn the_comparison_accepts_a_leading_v_and_reads_segments_as_numbers() {
        assert!(is_newer("1.4.1", "v1.5.0"));
        assert!(is_newer("v1.4.1", "1.5.0"));
        assert!(is_newer("v1.4.1", "V1.5.0"));
        assert!(!is_newer("1.4.1", "v1.4.1"));

        assert!(is_newer("1.9.9", "1.10.0"), "10 is greater than 9");
        assert!(is_newer("1.4.1", "1.4.10"));
        assert!(is_newer("1.99.0", "1.100.0"));
    }

    /// Anything that is not a clean three-part numeric version is not comparable, and an
    /// unparseable version must never be reported as an update.
    #[test]
    fn nothing_that_is_not_a_clean_three_part_version_is_newer() {
        let unparsable = [
            "",
            " ",
            "1.4",
            "1",
            "1.4.1.0",
            "1..4",
            "..",
            "v",
            "latest",
            "main",
            "1.5.0-rc1",
            "1.5.0+build",
            "1.5.0 beta",
            "one.two.three",
            "-1.5.0",
            "1.5.-1",
            "+1.5.0",
            "٠١.5.0",
            "99999999999999999999999.0.0",
        ];
        for value in unparsable {
            assert!(
                !is_newer("1.4.1", value),
                "{value:?} must not be reported as newer"
            );
            assert!(
                !is_newer(value, "1.4.1"),
                "{value:?} must not be usable as the installed version either"
            );
        }
    }

    /// Only the two known fields are read, from a body that also carries fields the check must not
    /// act on — including a download link pointing somewhere else entirely.
    #[test]
    fn only_the_tag_and_the_link_are_read_from_a_release_body() {
        let (tag_name, html_url) = parse_release(NEWER_BODY).expect("a valid body must parse");
        assert_eq!(tag_name, "v9.9.9");
        assert_eq!(
            html_url,
            "https://github.com/michaelocez/league-auto-accept/releases/tag/v9.9.9"
        );

        // Surrounding whitespace is not part of either value.
        assert_eq!(
            parse_release(
                r#"{"tag_name":" v9.9.9 ","html_url":" https://github.com/michaelocez/league-auto-accept/releases "}"#
            ),
            Some(("v9.9.9".to_string(), RELEASES_PAGE.to_string()))
        );
    }

    #[tokio::test]
    async fn a_body_missing_either_field_truncated_or_garbage_is_no_answer() {
        let unusable = [
            "",
            " ",
            "not json",
            "{}",
            r#"{"tag_name":"v9.9.9"}"#,
            r#"{"html_url":"https://github.com/michaelocez/league-auto-accept/releases"}"#,
            r#"{"tag_name":null,"html_url":"https://github.com/michaelocez/league-auto-accept/releases"}"#,
            r#"{"tag_name":5,"html_url":"https://github.com/michaelocez/league-auto-accept/releases"}"#,
            r#"{"tag_name":"v9.9.9","html_url":null}"#,
            r#"{"tag_name":"","html_url":"https://github.com/michaelocez/league-auto-accept/releases"}"#,
            r#"{"tag_name":"v9.9.9","html_url":""}"#,
            r#"{"tag_name":"v9.9.9","html_url":"https://github.com/michaelocez/league-auto-accept/releases""#,
            r#"[{"tag_name":"v9.9.9"}]"#,
        ];
        for body in unusable {
            assert!(
                parse_release(body).is_none(),
                "{body:?} must not parse as a release"
            );
            assert!(
                check_release(&FakeFetcher::answering(body)).await.is_none(),
                "{body:?} must not produce a notice"
            );
        }
    }

    /// The cap is a boundary, not a suggestion: a body at it is still an answer, and one byte over
    /// it is not — regardless of which fetcher produced the body.
    #[tokio::test]
    async fn a_body_at_the_cap_is_an_answer_and_one_byte_over_is_not() {
        let at_cap = padded_body(MAX_BODY_BYTES);
        assert_eq!(
            at_cap.len(),
            MAX_BODY_BYTES,
            "the fixture must sit on the cap"
        );
        assert!(
            check_release(&FakeFetcher::answering(at_cap))
                .await
                .is_some(),
            "a body at the cap must still be usable"
        );

        let over_cap = padded_body(MAX_BODY_BYTES + 1);
        assert_eq!(over_cap.len(), MAX_BODY_BYTES + 1);
        assert!(
            check_release(&FakeFetcher::answering(over_cap))
                .await
                .is_none(),
            "a body over the cap must be no answer"
        );
    }

    /// A valid answer naming a release newer than this build is reported, with its own page.
    #[tokio::test]
    async fn a_newer_release_is_reported_with_its_own_page() {
        let notice = check_release(&FakeFetcher::answering(NEWER_BODY))
            .await
            .expect("a newer release must be reported");

        assert_eq!(notice.version, "v9.9.9");
        assert_eq!(
            notice.url,
            "https://github.com/michaelocez/league-auto-accept/releases/tag/v9.9.9"
        );
    }

    /// An answer that is not newer — the same version, an older one, or a version that cannot be
    /// parsed — is reported as nothing at all. There is no "up to date" state to show.
    #[tokio::test]
    async fn an_answer_that_is_not_newer_is_reported_as_nothing() {
        let bodies = [
            format!(r#"{{"tag_name":"v{VERSION}","html_url":"{RELEASES_PAGE}"}}"#),
            r#"{"tag_name":"v1.0.0","html_url":"https://github.com/michaelocez/league-auto-accept/releases"}"#.to_string(),
            r#"{"tag_name":"latest","html_url":"https://github.com/michaelocez/league-auto-accept/releases"}"#.to_string(),
        ];
        for body in bodies {
            assert!(
                check_release(&FakeFetcher::answering(body)).await.is_none(),
                "a body that is not newer must produce no notice"
            );
        }
    }

    /// Every way the lookup can fail is the same "no answer": offline, refused, a timeout, a rate
    /// limit. It is a value, not a panic and not an error the UI would have to render.
    #[tokio::test]
    async fn every_lookup_failure_is_no_answer_rather_than_an_error() {
        let failures = [
            UpdateError::Connect,
            UpdateError::Tls,
            UpdateError::Timeout,
            UpdateError::Status(403),
            UpdateError::Status(429),
            UpdateError::Status(500),
            UpdateError::TooLarge,
            UpdateError::Read,
        ];
        for error in failures {
            assert!(
                check_release(&FakeFetcher::failing(error)).await.is_none(),
                "{error:?} must be no answer"
            );
        }
    }

    /// The guard: a link from the network is opened only when it is on this project's own GitHub
    /// path. Everything else falls back to the fixed releases page, including hosts that merely
    /// look similar.
    #[test]
    fn a_link_off_the_projects_github_path_falls_back_to_the_fixed_page() {
        let hostile = [
            "",
            "http://github.com/michaelocez/league-auto-accept/releases",
            "https://github.com/someone-else/league-auto-accept/releases",
            "https://github.com/michaelocez/league-auto-accept-evil/releases",
            "https://github.com.evil.test/michaelocez/league-auto-accept/releases",
            "https://evil.test/michaelocez/league-auto-accept/releases",
            "https://user:pass@github.com/michaelocez/league-auto-accept/releases",
            "javascript:alert(1)",
            "file:///C:/Windows/System32/cmd.exe",
        ];
        for url in hostile {
            assert_eq!(
                release_target(url),
                RELEASES_PAGE,
                "{url:?} must not be opened"
            );
        }

        for url in [
            "https://github.com/michaelocez/league-auto-accept/releases",
            "https://github.com/michaelocez/league-auto-accept/releases/tag/v9.9.9",
        ] {
            assert_eq!(
                release_target(url),
                url,
                "{url:?} is the project's own page"
            );
        }
    }

    /// The guard applied end to end: an answer whose link is off the project's path still produces
    /// a notice, but the notice points at the fixed page rather than at the link it was handed.
    #[tokio::test]
    async fn an_answer_cannot_send_the_browser_off_the_projects_own_page() {
        let body = r#"{"tag_name":"v9.9.9","html_url":"https://evil.test/league-auto-accept.exe"}"#;
        let notice = check_release(&FakeFetcher::answering(body))
            .await
            .expect("the version is still newer, so there is still something to say");

        assert_eq!(notice.version, "v9.9.9");
        assert_eq!(
            notice.url, RELEASES_PAGE,
            "the hostile link must not be the one offered"
        );
    }

    /// At most one check per session, however often Settings is opened.
    #[test]
    fn the_check_is_asked_for_once_per_session() {
        let mut gate = CheckGate::default();

        assert!(gate.begin(), "the first visit to Settings asks");
        for _ in 0..5 {
            assert!(!gate.begin(), "no later visit may ask again");
        }
    }

    /// Builds a valid release body of exactly `len` bytes by growing an ignored field, so the cap's
    /// boundary can be tested without relying on a magic string.
    fn padded_body(len: usize) -> String {
        let base = serde_json::json!({
            "tag_name": "v9.9.9",
            "html_url": RELEASES_PAGE,
            "padding": "",
        })
        .to_string();
        let extra = len.saturating_sub(base.len());
        // `extra` characters inside the padding value grow the body by exactly `extra`, wherever
        // the field happens to sit in the object.
        let padded = base.replace(
            r#""padding":"""#,
            &format!(r#""padding":"{}""#, "x".repeat(extra)),
        );
        assert_eq!(padded.len(), len, "the fixture must be exactly {len} bytes");
        padded
    }
}
