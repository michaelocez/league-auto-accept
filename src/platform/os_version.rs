//! Which Windows this process is running on, for the in-memory diagnostics report.
//!
//! `RtlGetVersion` is the only honest source available here. `GetVersionExW` reports the version the
//! application's manifest claims to support rather than the one it is running on — which is why it
//! is deprecated, and this binary carries no compatibility manifest — so the build number it would
//! return for anything since Windows 8.1 is a fixed lie. `windows-sys` declares `RtlGetVersion` in
//! its WDK namespace, so that is where this comes from.

use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

/// `STATUS_SUCCESS`: `RtlGetVersion` returns an NTSTATUS, not a BOOL.
const STATUS_SUCCESS: i32 = 0;

/// The first Windows 11 build. Windows 10 and 11 share a `10.0` version number, so this is the only
/// thing that tells them apart.
const WINDOWS_11_FIRST_BUILD: u32 = 22000;

/// A one-line description for a bug report: the Windows release where it can be named, the raw
/// version always, and the architecture this build is.
///
/// The raw version is included on purpose: a named release on its own would make a report
/// ambiguous, and a maintainer can do the naming themselves if this ever lags Windows.
pub fn description() -> String {
    match version() {
        Some((major, minor, build)) => format!(
            "{} ({major}.{minor}.{build}) · {}",
            release_name(major, minor, build),
            std::env::consts::ARCH
        ),
        // Nothing here is load-bearing for the app, so an unavailable version is reported rather
        // than turned into a failure.
        None => format!("Windows (version unavailable) · {}", std::env::consts::ARCH),
    }
}

/// The friendly name of a Windows version, when it is one this knows.
fn release_name(major: u32, minor: u32, build: u32) -> String {
    match (major, minor) {
        (10, 0) if build >= WINDOWS_11_FIRST_BUILD => "Windows 11".to_string(),
        (10, 0) => "Windows 10".to_string(),
        (6, 3) => "Windows 8.1".to_string(),
        (6, 2) => "Windows 8".to_string(),
        (6, 1) => "Windows 7".to_string(),
        _ => format!("Windows {major}.{minor}"),
    }
}

/// The real major/minor/build numbers, or `None` when the call is refused.
fn version() -> Option<(u32, u32, u32)> {
    // The size field must be set to the struct's own size before the call, which is what makes the
    // API's versioning work; everything else is written by Windows.
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a correctly sized, initialised `OSVERSIONINFOW` that says so in its own size
    // field, which is the API's only precondition; it writes nothing outside that struct.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status != STATUS_SUCCESS {
        return None;
    }
    Some((info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The known-answer half: the build number is what separates Windows 10 from Windows 11, and the
    /// boundary itself must land on 11.
    #[test]
    fn each_known_version_is_named_and_the_windows_11_boundary_is_the_build() {
        assert_eq!(release_name(10, 0, WINDOWS_11_FIRST_BUILD), "Windows 11");
        assert_eq!(
            release_name(10, 0, WINDOWS_11_FIRST_BUILD + 1),
            "Windows 11"
        );
        assert_eq!(
            release_name(10, 0, WINDOWS_11_FIRST_BUILD - 1),
            "Windows 10",
            "one build below the first Windows 11 build is still Windows 10"
        );
        assert_eq!(release_name(10, 0, 19045), "Windows 10");
        assert_eq!(release_name(6, 3, 9600), "Windows 8.1");
        assert_eq!(release_name(6, 2, 9200), "Windows 8");
        assert_eq!(release_name(6, 1, 7601), "Windows 7");
    }

    /// An unknown version is still reported, as itself rather than as a guess.
    #[test]
    fn an_unknown_version_is_reported_as_its_numbers() {
        assert_eq!(release_name(11, 1, 1), "Windows 11.1");
        assert_eq!(release_name(6, 0, 6002), "Windows 6.0");
    }

    /// The read itself, on the machine running the tests: it must answer, and the answer must be a
    /// real Windows version rather than something the struct's uninitialised tail could produce.
    #[test]
    fn this_process_reads_a_plausible_windows_version() {
        let (major, minor, build) = version().expect("RtlGetVersion answers on Windows");
        assert!(
            (6..=99).contains(&major),
            "implausible Windows major version {major}"
        );
        assert!(minor <= 9, "implausible Windows minor version {minor}");
        assert!(build >= 1000, "implausible Windows build {build}");
    }

    /// What the report actually carries: the release, the raw version and this build's
    /// architecture.
    #[test]
    fn the_description_names_the_release_the_version_and_the_architecture() {
        let description = description();
        assert!(description.starts_with("Windows"), "{description}");
        assert!(
            description.contains(std::env::consts::ARCH),
            "the architecture must be in it: {description}"
        );
        if let Some((major, minor, build)) = version() {
            assert!(
                description.contains(&format!("({major}.{minor}.{build})")),
                "the raw version must be in it: {description}"
            );
        }
    }
}
