//! League Client lockfile parsing.
//!
//! The lockfile is a colon-separated line: `processName:processId:port:password:protocol`.

use std::path::{Path, PathBuf};

/// Lockfiles larger than this are rejected.
pub const MAX_LOCKFILE_BYTES: u64 = 2048;
/// Maximum accepted password length.
pub const MAX_PASSWORD_LEN: usize = 512;
/// Maximum accepted process-name length.
pub const MAX_PROCESS_NAME_LEN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LcuCredentials {
    pub install_path: PathBuf,
    pub process_name: String,
    pub process_id: u64,
    pub port: u16,
    pub password: String,
}

impl LcuCredentials {
    /// Change-detection signature used to notice a restarted or replaced client
    /// (`processId:port:password`).
    pub fn signature(&self) -> String {
        format!("{}:{}:{}", self.process_id, self.port, self.password)
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

    Some(LcuCredentials {
        install_path: install_path.to_path_buf(),
        process_name: process_name.to_string(),
        process_id,
        port,
        password: password.to_string(),
    })
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
        assert_eq!(credentials.password, "local-secret");
        assert_eq!(credentials.signature(), "1234:54321:local-secret");
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
