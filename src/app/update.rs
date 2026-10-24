//! The application's shipped version, and the link to its releases.
//!
//! The version comes from Cargo's own compile-time metadata, so the line the Settings screen shows
//! is the number the binary was actually built with rather than a string that can drift from it.

/// The version this build reports. Compile-time, so it cannot disagree with `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The project's releases page — the fixed destination the version line opens.
///
/// Fixed on purpose: the version line must not be able to send the browser anywhere that arrived
/// over the network. When the release check finds a newer release it uses that release's own link,
/// but only after the link has passed a guard; this page is the fallback when it does not.
pub const RELEASES_PAGE: &str = "https://github.com/michaelocez/league-auto-accept/releases";

#[cfg(test)]
mod tests {
    use super::*;

    /// The line is only worth showing if the number in it is a real version, and the link is only
    /// safe to open if it is HTTPS on this project's own repositories.
    ///
    /// The build would fail on a `Cargo.toml` version that is not three numeric parts anyway — this
    /// pins that the UI's claim and the manifest cannot drift apart, including through a future
    /// `Cargo.toml` edit that cargo would still accept.
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
            RELEASES_PAGE.starts_with("https://github.com/michaelocez/league-auto-accept/"),
            "{RELEASES_PAGE} must be the project's own HTTPS page"
        );
    }
}
