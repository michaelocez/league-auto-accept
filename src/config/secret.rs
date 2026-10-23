//! Secret storage for the Discord webhook URL.
//!
//! The webhook URL is the only secret. On Windows it is protected with DPAPI (`CryptProtectData`);
//! a value the OS refuses to protect is never stored, so the secret is not silently downgraded to
//! cleartext — the caller is told instead.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// A persisted secret: `protection` is `"os"` (OS-encrypted) or `"plain"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSecret {
    pub protection: String,
    pub value: String,
}

/// A value the codec could not protect for storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretError;

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the operating system refused to protect the value")
    }
}

impl std::error::Error for SecretError {}

/// Encodes/decodes the webhook secret. Injectable so tests can use a deterministic codec.
pub trait SecretCodec: Send + Sync {
    /// Protects `value` for storage, or fails rather than handing back a cleartext value.
    fn encode(&self, value: &str) -> Result<StoredSecret, SecretError>;
    fn decode(&self, secret: &StoredSecret) -> String;
}

/// A no-protection codec: the host seam that lets this Windows-only crate be checked and tested
/// anywhere, and keeps its pure codec logic testable without DPAPI.
///
/// It must never be reachable in a Windows build. `default_secret_codec` selects it only under
/// `cfg(not(windows))`, and nothing else constructs it — storing the webhook in cleartext on the
/// machine that actually runs the app is exactly what the DPAPI refusal exists to prevent.
pub struct PlainSecretCodec;

impl SecretCodec for PlainSecretCodec {
    fn encode(&self, value: &str) -> Result<StoredSecret, SecretError> {
        Ok(StoredSecret {
            protection: "plain".into(),
            value: value.to_string(),
        })
    }

    fn decode(&self, secret: &StoredSecret) -> String {
        secret.value.clone()
    }
}

/// Returns the platform default codec (DPAPI on Windows, plain elsewhere).
pub fn default_secret_codec() -> Box<dyn SecretCodec> {
    #[cfg(windows)]
    {
        Box::new(DpapiSecretCodec)
    }
    #[cfg(not(windows))]
    {
        Box::new(PlainSecretCodec)
    }
}

/// DPAPI-backed codec. Uses the machine/user DPAPI scope, matching Electron `safeStorage`.
#[cfg(windows)]
pub struct DpapiSecretCodec;

#[cfg(windows)]
impl SecretCodec for DpapiSecretCodec {
    fn encode(&self, value: &str) -> Result<StoredSecret, SecretError> {
        Self::encode_protected(value, dpapi::protect)
    }

    fn decode(&self, secret: &StoredSecret) -> String {
        if secret.value.is_empty() {
            return String::new();
        }
        if secret.protection != "os" {
            return secret.value.clone();
        }
        let Ok(data) = base64::engine::general_purpose::STANDARD.decode(secret.value.as_bytes())
        else {
            return String::new();
        };
        match dpapi::unprotect(&data) {
            Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            None => String::new(),
        }
    }
}

#[cfg(windows)]
impl DpapiSecretCodec {
    /// Encodes through an injectable protector, so the refusal path is testable without making the
    /// real DPAPI call fail.
    fn encode_protected(
        value: &str,
        protect: impl Fn(&[u8]) -> Option<Vec<u8>>,
    ) -> Result<StoredSecret, SecretError> {
        // An empty webhook is not a secret, so it needs no protection and can always be removed.
        if value.is_empty() {
            return Ok(StoredSecret {
                protection: "os".into(),
                value: String::new(),
            });
        }
        match protect(value.as_bytes()) {
            Some(bytes) => Ok(StoredSecret {
                protection: "os".into(),
                value: base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
            None => Err(SecretError),
        }
    }
}

#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    fn empty_blob() -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        }
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        let mut out = empty_blob();
        let ok = unsafe {
            CryptProtectData(
                &blob(data),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut out,
            )
        };
        if ok == 0 {
            return None;
        }
        take_blob(out)
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let mut out = empty_blob();
        let ok = unsafe {
            CryptUnprotectData(
                &blob(data),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut out,
            )
        };
        if ok == 0 {
            return None;
        }
        take_blob(out)
    }

    /// Copies and frees a DPAPI output blob, rejecting the null or zero-length case.
    ///
    /// `std::slice::from_raw_parts` requires a non-null pointer even for an empty slice, so a blob
    /// the API reports without a buffer must not reach it.
    fn take_blob(out: CRYPT_INTEGER_BLOB) -> Option<Vec<u8>> {
        let bytes = if out.pbData.is_null() || out.cbData == 0 {
            None
        } else {
            // SAFETY: `out` was written by a successful DPAPI call, so `pbData` points at `cbData`
            // bytes the caller owns; they are copied before the buffer is freed exactly once below.
            Some(unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec())
        };
        if !out.pbData.is_null() {
            unsafe { LocalFree(out.pbData as *mut c_void) };
        }
        bytes
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_null_or_empty_output_blob_is_rejected_instead_of_sliced() {
            assert!(take_blob(empty_blob()).is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "https://discord.com/api/webhooks/12345678901234567/token";

    #[test]
    fn plain_codec_round_trips() {
        let codec = PlainSecretCodec;
        let stored = codec.encode(SAMPLE).expect("plain storage cannot fail");
        assert_eq!(stored.protection, "plain");
        assert_eq!(codec.decode(&stored), SAMPLE);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trips_and_does_not_store_plaintext() {
        let codec = DpapiSecretCodec;
        let stored = codec
            .encode(SAMPLE)
            .expect("DPAPI protects in a test process");
        assert_eq!(stored.protection, "os");
        assert!(!stored.value.contains("discord.com"));
        assert_eq!(codec.decode(&stored), SAMPLE);
    }

    /// The defect this guards: a failed `CryptProtectData` used to be reported as `protection:
    /// "plain"` with the URL in the clear. Refusing is the only safe answer.
    #[cfg(windows)]
    #[test]
    fn a_refused_protection_is_an_error_rather_than_cleartext() {
        let refused = DpapiSecretCodec::encode_protected(SAMPLE, |_| None);
        assert_eq!(refused, Err(SecretError));

        // An empty value is not a secret, so removal still works without touching DPAPI.
        let empty = DpapiSecretCodec::encode_protected("", |_| None)
            .expect("the empty webhook needs no protection");
        assert_eq!(empty.protection, "os");
        assert!(empty.value.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_decoding_invalid_data_returns_empty() {
        let codec = DpapiSecretCodec;
        assert_eq!(
            codec.decode(&StoredSecret {
                protection: "os".into(),
                value: "not-base64!!".into()
            }),
            ""
        );
        assert_eq!(
            codec.decode(&StoredSecret {
                protection: "os".into(),
                value: String::new()
            }),
            ""
        );
    }
}
