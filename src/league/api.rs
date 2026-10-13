//! LCU request construction and endpoint validation.
//!
//! The client is restricted to the endpoints the application actually uses: `is_allowed_lcu_endpoint`
//! is an exact match against `ALLOWED_LCU_ENDPOINTS` rather than a prefix check, so no other
//! `/lol-` path is reachable.

/// Every LCU endpoint the application may call.
pub const ALLOWED_LCU_ENDPOINTS: [&str; 4] = [
    crate::league::SUMMONER_ENDPOINT,
    crate::league::GAMEFLOW_ENDPOINT,
    crate::league::READY_CHECK_ENDPOINT,
    crate::league::READY_CHECK_ACCEPT_ENDPOINT,
];

/// The only HTTP methods the application needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        }
    }
}

/// A validated LCU request (method + allowed endpoint + optional JSON body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LcuRequest {
    pub(crate) method: HttpMethod,
    pub(crate) endpoint: String,
    pub(crate) body: Option<String>,
}

impl LcuRequest {
    /// Builds a request, rejecting unsupported methods/endpoints. The body, when present, must
    /// already be serialised JSON and is size-checked.
    pub(crate) fn new(
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
pub(crate) const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// Validates an LCU endpoint against the fixed allowlist.
pub(crate) fn is_allowed_lcu_endpoint(endpoint: &str) -> bool {
    ALLOWED_LCU_ENDPOINTS.contains(&endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::league;

    #[test]
    fn allows_only_the_endpoints_the_application_uses() {
        for endpoint in ALLOWED_LCU_ENDPOINTS {
            assert!(is_allowed_lcu_endpoint(endpoint), "{endpoint:?}");
        }
        assert_eq!(ALLOWED_LCU_ENDPOINTS.len(), 4);
    }

    #[test]
    fn rejects_lol_prefixed_paths_outside_the_allowlist() {
        let invalid = [
            "/lol-matchmaking/v1/ready-check/decline",
            "/lol-summoner/v1/current-summoner/extra",
            "/lol-gameflow/v1/session",
            "/lol-chat/v1/friends",
            "/lol-gameflow/v1/phase/",
        ];
        for value in invalid {
            assert!(
                !is_allowed_lcu_endpoint(value),
                "{value:?} should be rejected"
            );
        }
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
    fn exposes_only_the_methods_the_application_uses() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
    }
}
