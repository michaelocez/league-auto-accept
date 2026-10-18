//! The integrity level this process runs at, read from the Windows token API.
//!
//! Only the settings store's diagnostics use it, but it is Windows platform code rather than
//! persistence code, so it lives with the other Windows integration.

/// The integrity level of this process, as the last sub-authority of its integrity SID.
///
/// `0x1000` is low, `0x2000` is the level normal user programs run at.
pub(crate) fn process_integrity_level() -> Option<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

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
            let count = GetSidSubAuthorityCount(label.Label.Sid);
            if count.is_null() {
                None
            } else {
                let last = GetSidSubAuthority(label.Label.Sid, *count as u32 - 1);
                if last.is_null() {
                    None
                } else {
                    Some(*last)
                }
            }
        };
        CloseHandle(token);
        level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_runs_at_a_plausible_integrity_level() {
        // Exercises the token query behind the banner's restricted-launch note. A plausible level —
        // low through system — means the integrity SID was read correctly rather than through a
        // stale pointer, which is the failure this check exists to rule out.
        let level = process_integrity_level().expect("the process token has an integrity level");
        assert!(
            (0x1000..=0x4000).contains(&level),
            "implausible integrity level {level:#x}"
        );
    }
}
