//! Secret storage for the Discord webhook URL.
//!
//! The webhook URL is the only secret. On Windows it is protected with DPAPI
//! (`CryptProtectData`); if protection is unavailable it falls back to a plainly-stored value.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// A persisted secret: `protection` is `"os"` (OS-encrypted) or `"plain"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSecret {
    pub protection: String,
    pub value: String,
}

impl StoredSecret {
    pub fn is_plain(&self) -> bool {
        self.protection == "plain"
    }
}

/// Encodes/decodes the webhook secret. Injectable so tests can use a deterministic codec.
pub trait SecretCodec: Send + Sync {
    fn encode(&self, value: &str) -> StoredSecret;
    fn decode(&self, secret: &StoredSecret) -> String;
}

/// A no-protection codec (used on non-Windows and in tests).
pub struct PlainSecretCodec;

impl SecretCodec for PlainSecretCodec {
    fn encode(&self, value: &str) -> StoredSecret {
        StoredSecret {
            protection: "plain".into(),
            value: value.to_string(),
        }
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
    fn encode(&self, value: &str) -> StoredSecret {
        if value.is_empty() {
            return StoredSecret {
                protection: "os".into(),
                value: String::new(),
            };
        }
        match dpapi::protect(value.as_bytes()) {
            Some(bytes) => StoredSecret {
                protection: "os".into(),
                value: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
            None => StoredSecret {
                protection: "plain".into(),
                value: value.to_string(),
            },
        }
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
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as *mut c_void) };
        Some(bytes)
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
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as *mut c_void) };
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "https://discord.com/api/webhooks/12345678901234567/token";

    #[test]
    fn plain_codec_round_trips() {
        let codec = PlainSecretCodec;
        let stored = codec.encode(SAMPLE);
        assert!(stored.is_plain());
        assert_eq!(codec.decode(&stored), SAMPLE);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trips_and_does_not_store_plaintext() {
        let codec = DpapiSecretCodec;
        let stored = codec.encode(SAMPLE);
        assert_eq!(stored.protection, "os");
        assert!(!stored.value.contains("discord.com"));
        assert_eq!(codec.decode(&stored), SAMPLE);
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
