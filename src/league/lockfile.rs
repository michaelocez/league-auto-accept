//! League Client lockfile parsing.
//!
//! The lockfile is a colon-separated line: `processName:processId:port:password:protocol`.

use std::fmt;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use sha1::{Digest, Sha1};

/// Lockfiles larger than this are rejected.
pub const MAX_LOCKFILE_BYTES: u64 = 2048;
/// Maximum accepted password length.
pub const MAX_PASSWORD_LEN: usize = 512;
/// Maximum accepted process-name length.
pub const MAX_PROCESS_NAME_LEN: usize = 128;

/// Client credentials read from the lockfile. The password is private: it is only ever read
/// inside this module (to build the `Authorization` header and the change-detection digest).
#[derive(Clone, PartialEq, Eq)]
pub struct LcuCredentials {
    pub install_path: PathBuf,
    pub process_name: String,
    pub process_id: u64,
    pub port: u16,
    password: String,
}

impl LcuCredentials {
    pub fn new(
        install_path: PathBuf,
        process_name: String,
        process_id: u64,
        port: u16,
        password: String,
    ) -> Self {
        Self {
            install_path,
            process_name,
            process_id,
            port,
            password,
        }
    }

    /// The `Authorization` header value for LCU Basic auth.
    pub(crate) fn authorization(&self) -> String {
        let token = base64::engine::general_purpose::STANDARD.encode(format!(
            "{}:{}",
            super::LCU_USERNAME,
            self.password
        ));
        format!("Basic {token}")
    }

    /// Change-detection signature used to notice a restarted or replaced client
    /// (`processId:port:password`). Digested so the password is never recorded or compared in the
    /// clear.
    pub fn signature(&self) -> String {
        let mut hasher = Sha1::new();
        hasher.update(format!(
            "{}:{}:{}",
            self.process_id, self.port, self.password
        ));
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

impl fmt::Debug for LcuCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LcuCredentials")
            .field("install_path", &self.install_path)
            .field("process_name", &self.process_name)
            .field("process_id", &self.process_id)
            .field("port", &self.port)
            .field("password", &Redacted)
            .finish()
    }
}

struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Parses lockfile contents. Returns `None` for anything malformed or non-HTTPS.
pub fn parse_lockfile(contents: &str, install_path: &Path) -> Option<LcuCredentials> {
    let trimmed = contents.trim();
    let parts: Vec<&str> = trimmed.split(':').collect();
    if parts.len() != 5 {
        return None;
    }
    let process_name = parts[0];
    let process_id_text = parts[1];
    let port_text = parts[2];
    let password = parts[3];
    let protocol = parts[4];

    if !is_valid_process_name(process_name) {
        return None;
    }
    let process_id = process_id_text.parse::<u64>().ok().filter(|id| *id > 0)?;
    let port = port_text.parse::<u16>().ok().filter(|port| *port > 0)?;
    if password.is_empty() || password.len() > MAX_PASSWORD_LEN || contains_control_chars(password)
    {
        return None;
    }
    if !protocol.eq_ignore_ascii_case("https") {
        return None;
    }

    Some(LcuCredentials::new(
        install_path.to_path_buf(),
        process_name.to_string(),
        process_id,
        port,
        password.to_string(),
    ))
}

fn is_valid_process_name(value: &str) -> bool {
    let len = value.len();
    if len == 0 || len > MAX_PROCESS_NAME_LEN {
        return false;
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn contains_control_chars(value: &str) -> bool {
    value.chars().any(|c| {
        let code = c as u32;
        code <= 31 || code == 127
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_a_valid_lockfile() {
        let credentials = parse_lockfile(
            "LeagueClientUx:1234:54321:local-secret:https",
            Path::new("C:\\League"),
        )
        .expect("valid lockfile should parse");
        assert_eq!(credentials.process_name, "LeagueClientUx");
        assert_eq!(credentials.process_id, 1234);
        assert_eq!(credentials.port, 54321);
        // riot:local-secret
        assert_eq!(
            credentials.authorization(),
            "Basic cmlvdDpsb2NhbC1zZWNyZXQ="
        );
        assert_eq!(credentials.signature().len(), 40);
        assert!(!credentials.signature().contains("local-secret"));
    }

    #[test]
    fn signature_changes_when_the_client_restarts_or_reads_a_new_password() {
        let first = parse_lockfile("LeagueClientUx:1:1:pw:https", Path::new("C:\\League")).unwrap();
        let same = parse_lockfile("LeagueClientUx:1:1:pw:https", Path::new("C:\\League")).unwrap();
        let new_password =
            parse_lockfile("LeagueClientUx:1:1:other:https", Path::new("C:\\League")).unwrap();
        let new_port =
            parse_lockfile("LeagueClientUx:1:2:pw:https", Path::new("C:\\League")).unwrap();

        assert_eq!(first.signature(), same.signature());
        assert_ne!(first.signature(), new_password.signature());
        assert_ne!(first.signature(), new_port.signature());
    }

    #[test]
    fn debug_output_never_contains_the_password() {
        let credentials = parse_lockfile(
            "LeagueClientUx:1:1:local-secret:https",
            Path::new("C:\\League"),
        )
        .unwrap();
        let rendered = format!("{credentials:?}");
        assert!(!rendered.contains("local-secret"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn rejects_malformed_or_non_https_lockfiles() {
        let cases = [
            "LeagueClientUx:1234:70000:secret:https", // port out of range
            "LeagueClientUx:1234:0:secret:https",     // port zero
            "LeagueClientUx:1234:54321:secret:http",  // not https
            "LeagueClientUx:not-a-pid:54321:secret:https",
            "LeagueClientUx:1234:54321::https", // empty password
            "A B C:1234:54321:secret:https",    // invalid process name
            "only:three:parts",
            "a:b:c:d:e:f", // too many parts
        ];
        for case in cases {
            assert!(
                parse_lockfile(case, Path::new("C:\\League")).is_none(),
                "{case:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_control_characters_in_password() {
        let with_control = "LeagueClientUx:1:1:pass\u{0007}word:https";
        assert!(parse_lockfile(with_control, Path::new("C:\\League")).is_none());
    }

    #[test]
    fn accepts_uppercase_https_protocol() {
        let credentials = parse_lockfile("LeagueClientUx:1:1:pw:HTTPS", Path::new("C:\\League"));
        assert!(credentials.is_some());
    }
}
