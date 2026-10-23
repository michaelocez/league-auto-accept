//! The integrity level this process runs at, read from the Windows token API.
//!
//! Only the settings store's diagnostics use it, but it is Windows platform code rather than
//! persistence code, so it lives with the other Windows integration.

use std::ffi::c_void;
use std::sync::OnceLock;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Security::SID;

/// The revision every well-formed SID carries.
const SID_REVISION: u8 = 1;
/// `SID_MAX_SUB_AUTHORITIES`: no genuine SID has more, so a buffer that claims more is not one.
const SID_MAX_SUB_AUTHORITIES: u8 = 15;

/// How many times the token has actually been queried.
///
/// Only a test reads it: it is what shows the two diagnostics sites share one answer instead of
/// asking an immutable question twice.
#[cfg(test)]
static TOKEN_QUERIES: AtomicUsize = AtomicUsize::new(0);

/// The number of real token queries this process has made.
#[cfg(test)]
pub(crate) fn token_query_count() -> usize {
    // Relaxed is sufficient: a test-only counter that synchronises nothing and is compared with
    // itself on the thread that forced the single query.
    TOKEN_QUERIES.load(Ordering::Relaxed)
}

/// The integrity level of this process, as the last sub-authority of its integrity SID.
///
/// `0x1000` is low, `0x2000` is the level normal user programs run at.
pub(crate) fn process_integrity_level() -> Option<u32> {
    // A process cannot change the integrity level of its own token, so the first answer holds for
    // the process's whole life: the diagnostics log line and the banner share it instead of each
    // querying the token.
    static LEVEL: OnceLock<Option<u32>> = OnceLock::new();
    cached_level(&LEVEL, query_process_integrity_level)
}

/// The memoisation, split out so a test can hand it a fresh cell and count what it is asked for.
fn cached_level(cell: &OnceLock<Option<u32>>, query: impl FnOnce() -> Option<u32>) -> Option<u32> {
    *cell.get_or_init(query)
}

fn query_process_integrity_level() -> Option<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    #[cfg(test)]
    // Relaxed is sufficient: a test-only count that publishes nothing and orders nothing.
    TOKEN_QUERIES.fetch_add(1, Ordering::Relaxed);

    // Word-aligned because the label starts with pointers, and far larger than a label plus a SID.
    let mut buffer = [0u64; 32];
    let mut needed = 0u32;
    let mut token = std::ptr::null_mut();

    // SAFETY: the token handle is closed on every path that opened it, and the API is given a buffer
    // that is large enough for the label it writes, correctly aligned for the pointers inside it.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast(),
            std::mem::size_of_val(&buffer) as u32,
            &mut needed,
        );
        let level = if ok == 0 {
            None
        } else {
            let label = &*(buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>());
            integrity_level_from_sid(label.Label.Sid)
        };
        CloseHandle(token);
        level
    }
}

/// The level named by a mandatory-label SID (`S-1-16-…`), or `None` when `sid` is not one.
///
/// The structural checks run before any sub-authority is read, so a buffer that is not an integrity
/// SID is rejected rather than walked with whatever count its bytes happen to hold.
fn integrity_level_from_sid(sid: *mut c_void) -> Option<u32> {
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, SECURITY_MANDATORY_LABEL_AUTHORITY,
    };

    if sid.is_null() {
        return None;
    }
    // SAFETY: `sid` is non-null and points at a SID header — Windows wrote it into the token label,
    // or this module's tests built it. Only the header and its sub-authority array are read, and the
    // index is bounded by a count Windows reports and this function has already checked.
    unsafe {
        let header = sid.cast::<SID>();
        let count = GetSidSubAuthorityCount(sid);
        if count.is_null() {
            return None;
        }
        let count = *count;
        if (*header).Revision != SID_REVISION
            || (*header).IdentifierAuthority.Value != SECURITY_MANDATORY_LABEL_AUTHORITY.Value
            || count == 0
            || count > SID_MAX_SUB_AUTHORITIES
        {
            return None;
        }
        // The level is the *last* sub-authority, which is the only one a well-known label SID has.
        let last = GetSidSubAuthority(sid, u32::from(count) - 1);
        if last.is_null() {
            None
        } else {
            Some(*last)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use windows_sys::Win32::Security::{
        CreateWellKnownSid, WinHighLabelSid, WinLowLabelSid, WinMediumLabelSid, WinSystemLabelSid,
        WinUntrustedLabelSid, SECURITY_MANDATORY_LABEL_AUTHORITY, SECURITY_MAX_SID_SIZE,
    };

    /// A zeroed buffer with room for any well-known SID, as `CreateWellKnownSid` requires.
    struct SidBuffer([u64; 16]);

    impl SidBuffer {
        fn new() -> Self {
            Self([0; 16])
        }

        fn bytes(&mut self) -> &mut [u8] {
            // SAFETY: `self.0` is initialised, so every byte of it is a valid `u8` to write.
            unsafe { std::slice::from_raw_parts_mut(self.0.as_mut_ptr().cast(), 128) }
        }

        fn as_sid(&mut self) -> *mut c_void {
            self.0.as_mut_ptr().cast()
        }

        /// Writes a SID header with the given sub-authorities.
        fn write_sid(&mut self, revision: u8, authority: [u8; 6], sub_authorities: &[u32]) {
            let bytes = self.bytes();
            bytes[0] = revision;
            bytes[1] = sub_authorities.len() as u8;
            bytes[2..8].copy_from_slice(&authority);
            for (index, value) in sub_authorities.iter().enumerate() {
                bytes[8 + index * 4..12 + index * 4].copy_from_slice(&value.to_le_bytes());
            }
        }
    }

    /// Answers Windows builds itself, so a misread field or offset cannot pass: the reader has to
    /// return the level each known SID names — including `0`, which is a level and not a failure.
    #[test]
    fn a_known_label_sid_reads_as_its_own_level() {
        for (kind, expected) in [
            (WinUntrustedLabelSid, 0x0000u32),
            (WinLowLabelSid, 0x1000),
            (WinMediumLabelSid, 0x2000),
            (WinHighLabelSid, 0x3000),
            (WinSystemLabelSid, 0x4000),
        ] {
            let mut buffer = SidBuffer::new();
            let mut size = SECURITY_MAX_SID_SIZE;
            let built = unsafe {
                CreateWellKnownSid(kind, std::ptr::null_mut(), buffer.as_sid(), &mut size)
            };
            assert_ne!(built, 0, "Windows could not build well-known SID {kind}");
            assert_eq!(
                integrity_level_from_sid(buffer.as_sid()),
                Some(expected),
                "well-known SID {kind} reads as the wrong level"
            );
        }
    }

    /// The level is the last sub-authority, not the first: `S-1-16-0-8192` must read as medium
    /// rather than as the untrusted RID a first-sub-authority read would return.
    #[test]
    fn the_last_sub_authority_carries_the_level() {
        let mut buffer = SidBuffer::new();
        buffer.write_sid(
            SID_REVISION,
            SECURITY_MANDATORY_LABEL_AUTHORITY.Value,
            &[0x0000, 0x2000],
        );
        assert_eq!(integrity_level_from_sid(buffer.as_sid()), Some(0x2000));
    }

    /// The failure a plausible-range check cannot catch: bytes that are not an integrity SID must be
    /// rejected, rather than read as a level that happens to look reasonable.
    #[test]
    fn a_buffer_that_is_not_an_integrity_sid_is_rejected() {
        assert_eq!(
            integrity_level_from_sid(std::ptr::null_mut()),
            None,
            "a null SID"
        );

        // All zero: the wrong revision and a zero count as well as the wrong authority.
        let mut buffer = SidBuffer::new();
        assert_eq!(
            integrity_level_from_sid(buffer.as_sid()),
            None,
            "a zeroed buffer"
        );

        // Well formed except for the authority: 0x2000 here would be in range and wrong.
        buffer.write_sid(SID_REVISION, [0, 0, 0, 0, 0, 0], &[0x2000]);
        assert_eq!(
            integrity_level_from_sid(buffer.as_sid()),
            None,
            "a non-mandatory-label authority"
        );

        // The right authority, but a sub-authority count no SID can have.
        buffer.write_sid(
            SID_REVISION,
            SECURITY_MANDATORY_LABEL_AUTHORITY.Value,
            &[0x2000; 16],
        );
        assert_eq!(
            integrity_level_from_sid(buffer.as_sid()),
            None,
            "an impossible sub-authority count"
        );

        // The right authority and count, but a revision Windows never writes.
        buffer.write_sid(2, SECURITY_MANDATORY_LABEL_AUTHORITY.Value, &[0x2000]);
        assert_eq!(
            integrity_level_from_sid(buffer.as_sid()),
            None,
            "an unknown SID revision"
        );
    }

    /// The strongest assertion available for *this* process: its level is one of the five Windows
    /// defines. Which of them is correct depends on how the process was launched, so that part is
    /// not knowable here; the known-answer tests above cover the read itself.
    #[test]
    fn the_process_runs_at_a_documented_integrity_level() {
        let level = process_integrity_level().expect("the process token has an integrity level");
        assert!(
            matches!(level, 0x0000 | 0x1000 | 0x2000 | 0x3000 | 0x4000),
            "implausible integrity level {level:#x}"
        );
    }

    /// The measurement behind the cost reduction: the log line and the banner that reports the same
    /// refusal must not each query the token.
    #[test]
    fn the_token_is_queried_once_for_the_whole_process() {
        // Settle the cache first, so whether another test has already populated it cannot decide
        // this one — and after this returns, no further query is possible for the process.
        let _ = process_integrity_level();
        let before = token_query_count();

        let _ = process_integrity_level();
        let _ = process_integrity_level();

        assert_eq!(
            token_query_count(),
            before,
            "the process token was queried again for an answer that cannot change"
        );
    }

    /// The memoisation itself, on a cell this test owns, so it does not depend on the cache above.
    #[test]
    fn a_cached_answer_is_not_asked_for_twice() {
        let cell: OnceLock<Option<u32>> = OnceLock::new();
        let calls = Cell::new(0);
        let count = || {
            calls.set(calls.get() + 1);
            Some(0x2000)
        };

        assert_eq!(cached_level(&cell, count), Some(0x2000));
        assert_eq!(
            cached_level(&cell, || {
                calls.set(calls.get() + 1);
                Some(0x1000)
            }),
            Some(0x2000)
        );
        assert_eq!(calls.get(), 1, "the second answer was queried for");
    }
}
