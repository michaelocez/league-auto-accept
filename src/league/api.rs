//! LCU request construction and endpoint validation.
//!
//! Only endpoints under `/lol-` are ever reachable; the application itself uses only the fixed
//! endpoints in `league::mod`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }
}

/// A validated LCU request (method + allowed endpoint + optional JSON body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LcuRequest {
    pub method: HttpMethod,
    pub endpoint: String,
    pub body: Option<String>,
}

impl LcuRequest {
    /// Builds a request, rejecting unsupported methods/endpoints. The body, when present, must
    /// already be serialised JSON and is size-checked.
    pub fn new(
        method: HttpMethod,
        endpoint: &str,
        body: Option<String>,
    ) -> Result<Self, LcuRequestError> {
        if !is_allowed_lcu_endpoint(endpoint) {
            return Err(LcuRequestError::UnsupportedEndpoint);
        }
        if let Some(body) = &body {
            if body.len() > MAX_REQUEST_BYTES {
                return Err(LcuRequestError::BodyTooLarge);
            }
        }
        Ok(Self {
            method,
            endpoint: endpoint.to_string(),
            body,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LcuRequestError {
    UnsupportedEndpoint,
    BodyTooLarge,
}

impl std::fmt::Display for LcuRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LcuRequestError::UnsupportedEndpoint => write!(f, "unsupported LCU endpoint"),
            LcuRequestError::BodyTooLarge => {
                write!(f, "LCU request body exceeded the supported size limit")
            }
        }
    }
}

impl std::error::Error for LcuRequestError {}

/// Maximum request body size (1 MiB).
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Maximum endpoint length.
pub const MAX_ENDPOINT_LEN: usize = 4096;

/// Validates an LCU endpoint.
pub fn is_allowed_lcu_endpoint(endpoint: &str) -> bool {
    if !endpoint.starts_with("/lol-") || endpoint.len() > MAX_ENDPOINT_LEN {
        return false;
    }
    if contains_control_chars(endpoint) {
        return false;
    }
    if endpoint
        .chars()
        .any(|c| c.is_ascii_whitespace() || c == '\\' || c == '#')
    {
        return false;
    }
    if contains_percent_encoded_separator(endpoint) {
        return false;
    }
    // Reject absolute or protocol-relative forms.
    if endpoint.starts_with("//") || looks_like_scheme(endpoint) {
        return false;
    }
    true
}

fn contains_control_chars(value: &str) -> bool {
    value.chars().any(|c| {
        let code = c as u32;
        code <= 31 || code == 127
    })
}

fn contains_percent_encoded_separator(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] == b'%' {
            let a = bytes[index + 1].to_ascii_lowercase();
            let b = bytes[index + 2].to_ascii_lowercase();
            if a == b'2' && (b == b'e' || b == b'f') || a == b'5' && b == b'c' {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn looks_like_scheme(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::league;

    #[test]
    fn allows_league_client_api_paths() {
        assert!(is_allowed_lcu_endpoint(league::SUMMONER_ENDPOINT));
        assert!(is_allowed_lcu_endpoint(league::GAMEFLOW_ENDPOINT));
        assert!(is_allowed_lcu_endpoint(league::READY_CHECK_ACCEPT_ENDPOINT));
    }

    #[test]
    fn rejects_external_protocol_relative_and_malformed_paths() {
        let invalid = [
            "https://example.test/lol-summoner/v1/current-summoner",
            "//example.test/lol-summoner/v1/current-summoner",
            "/lol-test/%2e%2e/secret",
            "/lol-test/%2f/etc",
            "/lol-test/%5cwindows",
            "/riotclient/command-line-args",
            "/lol-gameflow/v1/phase extra",
            "/lol-gameflow/v1/phase\\x",
            "/lol-gameflow/v1/phase#frag",
        ];
        for value in invalid {
            assert!(
                !is_allowed_lcu_endpoint(value),
                "{value:?} should be rejected"
            );
        }
    }

    #[test]
    fn builds_requests_and_rejects_invalid_ones() {
        assert!(LcuRequest::new(HttpMethod::Get, league::GAMEFLOW_ENDPOINT, None).is_ok());
        assert_eq!(
            LcuRequest::new(HttpMethod::Post, "/not-lol/thing", None),
            Err(LcuRequestError::UnsupportedEndpoint)
        );
        let oversized = "x".repeat(MAX_REQUEST_BYTES + 1);
        assert_eq!(
            LcuRequest::new(
                HttpMethod::Post,
                league::READY_CHECK_ACCEPT_ENDPOINT,
                Some(oversized)
            ),
            Err(LcuRequestError::BodyTooLarge)
        );
    }

    #[test]
    fn allows_all_documented_methods() {
        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            assert!(LcuRequest::new(method, league::GAMEFLOW_ENDPOINT, None).is_ok());
        }
    }
}
