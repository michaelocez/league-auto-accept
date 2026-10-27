//! In-memory diagnostics: the recent records of this session, kept so a failure in the field can be
//! read at all.
//!
//! The shipped release is a GUI-subsystem binary whose stderr does not exist, so a `log::error!` in
//! the field is invisible. The owner's decision is that this application creates **no** log file:
//! instead the last records live in a bounded buffer, and Settings shows them only once something at
//! error level has actually been recorded, with a button that copies a report for a GitHub issue.
//!
//! Three properties hold that arrangement together:
//!
//! * Nothing is written anywhere but the buffer. A debug build still forwards every record to
//!   `env_logger`'s stderr exactly as it always has; a release build deliberately stops at the
//!   buffer, and the buffer dies with the process.
//! * Every record is redacted on the way in, so the report cannot carry the Discord webhook URL
//!   (whose path *is* the token), an LCU `Authorization` credential, or anything else listed in
//!   [`redact`] — however the text reached the logger.
//! * A record identical to the one made just before it — the same level, and the same text *after*
//!   redaction — is folded into that entry and counted rather than appended as a second line. Two
//!   layers recording one failure are two different records (their module prefixes differ) and stay
//!   two lines; one line recorded repeatedly in a row reads as one line that says how often it
//!   happened.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::app::update::VERSION;

/// How many records are kept at most.
pub const MAX_RECORDS: usize = 256;
/// How many bytes of record text are kept at most, whichever comes first with [`MAX_RECORDS`].
pub const MAX_TOTAL_BYTES: usize = 32 * 1024;
/// The longest single record kept, so one enormous record cannot flush the whole buffer.
pub const MAX_RECORD_BYTES: usize = 2048;
/// The most consecutive identical records one entry accounts for.
///
/// The count exists so a repeated line does not become repeated lines, and it stops here so a line
/// repeated in a loop cannot become a page of digits either. An entry at this count is open-ended:
/// the report says "1000+" rather than claiming to know a total it stopped counting.
pub const MAX_REPEATS: u32 = 1000;

/// What the end of a truncated record says.
const TRUNCATION_MARKER: &str = "… (truncated)";
/// What a redacted value is replaced with.
const REDACTED: &str = "<redacted>";
/// What the account-name segment of a user-profile path is replaced with.
const REDACTED_USER: &str = "<user>";

/// One retained record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The level the record was made at, so the buffer can answer "did anything fail?".
    pub level: log::Level,
    /// The record's module and message, already redacted and already bounded.
    pub text: String,
    /// How many consecutive identical records this entry stands for: at least one, and never more
    /// than [`MAX_REPEATS`].
    pub repeats: u32,
}

impl Entry {
    /// What the report appends to an entry that stands for more than one identical record.
    ///
    /// Empty when the entry was recorded once, so the marker can only ever appear where a count
    /// genuinely exists. It goes after the entry's level and text, neither of which it changes; it
    /// is bounded by [`MAX_REPEATS`], so a repeated line cannot lengthen the report without limit;
    /// and a count at the cap is written open-ended ("1000+"), which is what the stored number
    /// actually knows.
    fn repeat_note(&self) -> String {
        if self.repeats <= 1 {
            return String::new();
        }
        if self.repeats >= MAX_REPEATS {
            return format!(" [repeated {MAX_REPEATS}+ times]");
        }
        format!(" [repeated {} times]", self.repeats)
    }
}

#[derive(Debug, Default)]
struct Inner {
    records: VecDeque<Entry>,
    /// The sum of the retained records' text lengths, so the byte cap needs no second pass.
    bytes: usize,
}

/// The bounded store of this process's recent records.
///
/// Shared and mutable, so a record can be made from any thread — the League service and the
/// notification sender both run off the UI thread.
#[derive(Debug, Default)]
pub struct RecordBuffer {
    inner: Mutex<Inner>,
}

impl RecordBuffer {
    /// Retains one record, redacted, bounded, and folded into its predecessor when identical.
    ///
    /// Redaction and truncation happen here rather than at either end, so no caller can put text
    /// into the buffer that came from somewhere else: the logger is not the only possible writer.
    /// The comparison against the newest entry is made on this redacted text, so a secret is never
    /// what decides whether two records are the same one.
    pub fn push(&self, level: log::Level, text: &str) {
        let text = truncate(&redact(text));
        let mut inner = self.lock();
        // Only the entry just made, never the whole buffer: a failure that repeats later, after
        // other records, is real history whose order matters and keeps its own entry.
        if let Some(newest) = inner.records.back_mut() {
            if newest.level == level && newest.text == text {
                newest.repeats = newest.repeats.saturating_add(1).min(MAX_REPEATS);
                return;
            }
        }
        inner.bytes += text.len();
        inner.records.push_back(Entry {
            level,
            text,
            repeats: 1,
        });
        // Oldest first: the newest records are the ones worth keeping.
        while inner.records.len() > MAX_RECORDS || inner.bytes > MAX_TOTAL_BYTES {
            let Some(evicted) = inner.records.pop_front() else {
                break;
            };
            inner.bytes -= evicted.text.len();
        }
    }

    /// Whether anything at error level has been recorded this session.
    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    /// How many records were made at error level.
    pub fn error_count(&self) -> usize {
        self.entries()
            .iter()
            .filter(|entry| entry.level == log::Level::Error)
            .count()
    }

    /// The most recent error-level record, if there is one.
    pub fn latest_error(&self) -> Option<String> {
        self.entries()
            .into_iter()
            .rev()
            .find(|entry| entry.level == log::Level::Error)
            .map(|entry| entry.text)
    }

    /// Every retained record, oldest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.lock().records.iter().cloned().collect()
    }

    /// How many records are retained.
    pub fn len(&self) -> usize {
        self.lock().records.len()
    }

    /// Whether nothing has been retained.
    pub fn is_empty(&self) -> bool {
        self.lock().records.is_empty()
    }

    /// How many bytes of record text are retained.
    pub fn bytes(&self) -> usize {
        self.lock().bytes
    }

    /// The plain-text report for a GitHub issue: which build, which Windows, and the record tail.
    ///
    /// The tail is exactly what the buffer holds, which is already redacted, so this text can be
    /// pasted into a public issue. Nothing here is escaped: it is meant to be read as it stands.
    /// Each line is the record's level and its own text; an entry that stands for several
    /// consecutive identical records additionally ends in a `[repeated N times]` marker, so the
    /// count survives without the level or the message being touched.
    pub fn report(&self) -> String {
        let entries = self.entries();
        let errors = self.error_count();
        let mut lines = Vec::with_capacity(entries.len() + 4);
        lines.push(format!("League Auto Accept {VERSION}"));
        lines.push(crate::platform::os_version::description());
        lines.push(format!(
            "{} record(s) this session, {errors} at error level.",
            entries.len()
        ));
        lines.push("--- recent log records, oldest first ---".to_string());
        lines.extend(
            entries
                .iter()
                .map(|entry| format!("[{}] {}{}", entry.level, entry.text, entry.repeat_note())),
        );
        format!("{}\n", lines.join("\n"))
    }

    /// A record can be made from a thread that panicked earlier; the buffer is a diagnostic aid, so
    /// it is never worth failing over a poisoned lock.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// The process's record buffer, created on first use.
pub fn buffer() -> &'static Arc<RecordBuffer> {
    PROCESS.get_or_init(|| Arc::new(RecordBuffer::default()))
}

/// Installs the process's single `log` logger over [`buffer`].
///
/// Idempotent, so the caller does not have to care whether it has already run. The filter is the
/// application's existing one — `RUST_LOG`, defaulting to `info` — so `env_logger`'s parsing is
/// reused rather than reimplemented.
pub fn install() {
    LOGGER.get_or_init(|| {
        let filter =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                .build();
        let level = filter.filter();
        let logger = DiagnosticsLogger {
            inner: filter,
            buffer: buffer().clone(),
        };
        // Only one `log` logger can be installed per process, and this is the only place that tries.
        let _ = log::set_boxed_logger(Box::new(logger));
        log::set_max_level(level);
    });
}

/// Whether anything at error level has been recorded this session.
pub fn has_errors() -> bool {
    buffer().has_errors()
}

/// How many records were made at error level this session.
pub fn error_count() -> usize {
    buffer().error_count()
}

/// The most recent error-level record, if there is one.
pub fn latest_error() -> Option<String> {
    buffer().latest_error()
}

/// The report the Copy-diagnostics button puts on the clipboard.
pub fn report() -> String {
    buffer().report()
}

/// The logger: `env_logger`'s filter and formatting, with one buffer in front of it.
struct DiagnosticsLogger {
    inner: env_logger::Logger,
    buffer: Arc<RecordBuffer>,
}

impl log::Log for DiagnosticsLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        // The same filter `env_logger` would apply, so the buffer agrees with what a developer would
        // have seen on stderr.
        if !self.inner.matches(record) {
            return;
        }
        // Retained first: a record made just before a crash is the one worth having.
        self.buffer.push(
            record.level(),
            &format!("{}: {}", record.target(), record.args()),
        );

        // The developer's stderr, exactly as this application has always produced it — and only
        // there. A release build is a GUI-subsystem binary with nowhere to write, and by the owner's
        // decision this application writes nothing anywhere, so the release path ends here.
        #[cfg(debug_assertions)]
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Every shape that must not survive into the buffer.
///
/// Three, and deliberately only three — a report that redacted indiscriminately would hide the facts
/// a maintainer needs:
///
/// * a Discord webhook URL, whose *path* is the secret: `/api/webhooks/<id>/<token>`. The host and
///   the `/webhooks/` marker are kept, because they are what identifies the line.
/// * an HTTP `Authorization: Basic …` value, which is the LCU credential.
/// * the account-name segment of a Windows user-profile path, `C:\Users\<name>\…`. This is not a
///   secret, but it is the user's own account name going into a public GitHub issue, and it carries
///   no diagnostic value: the app-relative path is the fact that matters.
pub fn redact(text: &str) -> String {
    // Per line, because an `Authorization` header's value ends where its line does.
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&redact_user_profile_segment(&redact_authorization_value(
            &redact_runs_after(line, "webhooks/", is_webhook_path_character),
        )));
    }
    out
}

/// Replaces everything after `marker` that still looks like part of the same value.
fn redact_runs_after(line: &str, marker: &str, keep: impl Fn(char) -> bool) -> String {
    // ASCII-lowercasing preserves byte offsets, so a match found in `lower` indexes `line`.
    let lower = line.to_ascii_lowercase();
    let marker = marker.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(&marker) {
        let start = cursor + offset + marker.len();
        out.push_str(&line[cursor..start]);
        let end = run_end(&line[start..], &keep);
        if end > 0 {
            out.push_str(REDACTED);
        }
        cursor = start + end;
    }
    out.push_str(&line[cursor..]);
    out
}

/// How much of `rest` looks like the value a marker introduced.
fn run_end(rest: &str, keep: &impl Fn(char) -> bool) -> usize {
    let mut end = 0;
    for (index, character) in rest.char_indices() {
        if !keep(character) {
            break;
        }
        end = index + character.len_utf8();
    }
    end
}

/// Redacts the value of an HTTP `Authorization: Basic …` header, and nothing else on the line.
fn redact_authorization_value(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let Some(offset) = lower.find("authorization:") else {
        return line.to_string();
    };
    let value_start = offset + "authorization:".len();
    let (head, tail) = line.split_at(value_start);
    format!(
        "{head}{}",
        redact_runs_after(tail, "basic ", is_base64_character)
    )
}

/// The path segment that names a Windows user's profile directory, matched case-insensitively.
const USER_PROFILE_SEGMENT: &str = "users";

/// Redacts the account-name segment of a Windows user-profile path, and nothing else.
///
/// `C:\Users\Michael\AppData\…` becomes `C:\Users\<user>\AppData\…`, with either separator accepted
/// on either side of the name. The word has to be a whole path segment — introduced by a separator
/// and followed by one — so prose that merely mentions users, or a segment such as `PowerUsers`, is
/// left as it was written. The marker itself is kept, so the line still says which directory it is.
fn redact_user_profile_segment(line: &str) -> String {
    // ASCII-lowercasing preserves byte offsets, so a match found in `lower` indexes `line`.
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(USER_PROFILE_SEGMENT) {
        let marker_start = cursor + offset;
        let marker_end = marker_start + USER_PROFILE_SEGMENT.len();
        // The word is only a path segment when a separator introduces it and one follows it.
        let introduced_by_separator = line[..marker_start]
            .chars()
            .next_back()
            .is_some_and(is_path_separator);
        let followed_by_separator = line[marker_end..]
            .chars()
            .next()
            .is_some_and(is_path_separator);
        if !introduced_by_separator || !followed_by_separator {
            // Prose, or the word inside a longer segment: keep it exactly as it was written.
            out.push_str(&line[cursor..marker_end]);
            cursor = marker_end;
            continue;
        }
        // The matched separator is ASCII, so one byte past the marker is a character boundary. The
        // name then runs to the next separator, or to the end of the line.
        let name_start = marker_end + 1;
        let name_end = name_start + run_end(&line[name_start..], &is_not_path_separator);
        if name_end == name_start {
            // Nothing between the separators to replace.
            out.push_str(&line[cursor..name_start]);
            cursor = name_start;
            continue;
        }
        out.push_str(&line[cursor..name_start]);
        out.push_str(REDACTED_USER);
        cursor = name_end;
    }
    out.push_str(&line[cursor..]);
    out
}

/// Whether a character separates path segments. A formatted Windows path can arrive with either.
fn is_path_separator(character: char) -> bool {
    matches!(character, '\\' | '/')
}

/// The complement of [`is_path_separator`], so the account name can be scanned up to the next one.
fn is_not_path_separator(character: char) -> bool {
    !is_path_separator(character)
}

/// The characters a Discord webhook path can be made of. The token's own alphabet is a subset; what
/// matters is that the run stops at whitespace, a quote or any punctuation that ends it.
fn is_webhook_path_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '~' | '/')
}

/// The characters of a base64 value, which is what a Basic credential is.
fn is_base64_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=')
}

/// Bounds one record on a character boundary, marking where it was cut.
fn truncate(text: &str) -> String {
    if text.len() <= MAX_RECORD_BYTES {
        return text.to_string();
    }
    let mut end = MAX_RECORD_BYTES - TRUNCATION_MARKER.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_MARKER}", &text[..end])
}

static PROCESS: OnceLock<Arc<RecordBuffer>> = OnceLock::new();
static LOGGER: OnceLock<()> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    /// The buffer the tests own, so none of them depends on the process buffer's contents.
    fn buffer() -> RecordBuffer {
        RecordBuffer::default()
    }

    #[test]
    fn the_oldest_record_is_dropped_once_the_buffer_is_full() {
        let buffer = buffer();
        for index in 0..MAX_RECORDS + 5 {
            buffer.push(log::Level::Info, &format!("record {index}"));
        }

        assert_eq!(buffer.len(), MAX_RECORDS, "the cap is the record count");
        let entries = buffer.entries();
        assert_eq!(entries.first().unwrap().text, "record 5", "oldest dropped");
        assert_eq!(
            entries.last().unwrap().text,
            format!("record {}", MAX_RECORDS + 4),
            "newest kept"
        );
        assert!(
            !entries.iter().any(|entry| entry.text == "record 0"),
            "the very first record must be gone"
        );
    }

    #[test]
    fn a_long_record_is_truncated_to_the_per_record_cap() {
        let buffer = buffer();
        let head = "h".repeat(64);
        let long = format!("{head}{}", "x".repeat(16 * 1024));
        buffer.push(log::Level::Error, &long);

        let entries = buffer.entries();
        assert_eq!(entries.len(), 1);
        let text = &entries[0].text;
        assert!(
            text.len() <= MAX_RECORD_BYTES,
            "a record must not exceed its own cap, got {} bytes",
            text.len()
        );
        assert!(text.ends_with(TRUNCATION_MARKER), "{text}");
        assert!(text.starts_with(&head), "the beginning is what is kept");
    }

    /// The byte cap is what bounds a buffer of large records: 20 records of 2 KiB are far more than
    /// the cap, and fewer than [`MAX_RECORDS`] must be kept.
    ///
    /// The records are genuinely different ones, so this keeps testing the byte cap rather than the
    /// collapsing of identical records — and each is exactly [`MAX_RECORD_BYTES`], prefix included,
    /// so the retained byte total stays a whole number of records.
    #[test]
    fn the_byte_cap_bounds_the_buffer_before_the_record_count_does() {
        let buffer = buffer();
        for index in 0..20 {
            let record = format!("{index:02}{}", "y".repeat(MAX_RECORD_BYTES - 2));
            buffer.push(log::Level::Warn, &record);
        }

        assert!(
            buffer.bytes() <= MAX_TOTAL_BYTES,
            "the byte cap must hold, got {} bytes",
            buffer.bytes()
        );
        assert!(
            buffer.len() < MAX_RECORDS,
            "the byte cap, not the record cap, must be what bounded this: {} records",
            buffer.len()
        );
        assert_eq!(
            buffer.bytes(),
            buffer.len() * MAX_RECORD_BYTES,
            "the byte total must be the retained records' own bytes"
        );
    }

    /// The shape a pasted report should have: the same level and the same text, recorded twice in a
    /// row, is one entry that says how many records it stands for — with nothing else altered.
    #[test]
    fn two_identical_consecutive_records_collapse_into_one_entry_that_says_two() {
        let buffer = buffer();
        buffer.push(log::Level::Error, "settings save refused");
        buffer.push(log::Level::Error, "settings save refused");

        let entries = buffer.entries();
        assert_eq!(entries.len(), 1, "the second record is the first one again");
        assert_eq!(
            entries[0].level,
            log::Level::Error,
            "the level is not altered"
        );
        assert_eq!(
            entries[0].text, "settings save refused",
            "the message is not altered"
        );
        assert_eq!(
            entries[0].repeats, 2,
            "the count is the records, not the lines"
        );
        assert_eq!(
            buffer.bytes(),
            "settings save refused".len(),
            "no second copy of the text is retained"
        );
        assert_eq!(
            buffer.latest_error().as_deref(),
            Some("settings save refused"),
            "the Settings preview reads the record's own text, without the report's marker"
        );

        let report = buffer.report();
        assert!(
            report.contains("[ERROR] settings save refused [repeated 2 times]"),
            "one line, and it says it happened twice: {report}"
        );
    }

    #[test]
    fn three_identical_consecutive_records_collapse_and_count_three() {
        let buffer = buffer();
        for _ in 0..3 {
            buffer.push(log::Level::Warn, "retrying the accept");
        }

        let entries = buffer.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].repeats, 3);
        let report = buffer.report();
        assert!(
            report.contains("[WARN] retrying the accept [repeated 3 times]"),
            "{report}"
        );
    }

    /// A collapsed entry is still one entry: the record cap evicts it like any other, so the count
    /// cannot become a way of holding on to records the cap is supposed to drop.
    #[test]
    fn the_record_cap_still_evicts_even_with_a_collapsed_entry() {
        let buffer = buffer();
        for _ in 0..50 {
            buffer.push(log::Level::Warn, "the repeated warning");
        }
        let collapsed = buffer.entries();
        assert_eq!(collapsed.len(), 1, "the 50 identical records are one entry");
        assert_eq!(collapsed[0].repeats, 50);

        for index in 0..MAX_RECORDS {
            buffer.push(log::Level::Info, &format!("distinct record {index}"));
        }

        let entries = buffer.entries();
        assert_eq!(
            entries.len(),
            MAX_RECORDS,
            "the record cap is exactly the cap"
        );
        assert_eq!(
            buffer.bytes(),
            entries.iter().map(|entry| entry.text.len()).sum::<usize>(),
            "the byte total counts only what is retained"
        );
        assert_eq!(
            entries.last().unwrap().text,
            format!("distinct record {}", MAX_RECORDS - 1),
            "the newest record is the one kept"
        );
        assert!(
            !entries
                .iter()
                .any(|entry| entry.text == "the repeated warning"),
            "the collapsed entry, being the oldest, is what the cap evicted"
        );
    }

    /// A repeat separated by other records is history, not noise: collapsing must not merge it with
    /// an earlier same-looking line or move it out of order.
    #[test]
    fn non_consecutive_duplicates_stay_separate_entries() {
        let buffer = buffer();
        buffer.push(log::Level::Error, "same failure");
        buffer.push(log::Level::Info, "something else happened");
        buffer.push(log::Level::Error, "same failure");

        let entries = buffer.entries();
        assert_eq!(entries.len(), 3, "the ordering is the history");
        assert_eq!(entries[0].repeats, 1);
        assert_eq!(entries[1].text, "something else happened");
        assert_eq!(entries[2].repeats, 1, "the later repeat is its own entry");

        let report = buffer.report();
        assert_eq!(
            report.matches("[ERROR] same failure").count(),
            2,
            "and it is printed twice: {report}"
        );
        assert!(
            !report.contains("repeated"),
            "nothing here collapsed: {report}"
        );
    }

    #[test]
    fn the_same_text_at_different_levels_stays_two_entries() {
        let buffer = buffer();
        buffer.push(log::Level::Info, "the same words");
        buffer.push(log::Level::Error, "the same words");

        assert_eq!(buffer.entries().len(), 2, "the level is part of the record");
        let report = buffer.report();
        assert!(report.contains("[INFO] the same words"), "{report}");
        assert!(report.contains("[ERROR] the same words"), "{report}");
        assert!(!report.contains("repeated"), "{report}");
    }

    #[test]
    fn different_text_at_the_same_level_stays_two_entries() {
        let buffer = buffer();
        buffer.push(log::Level::Error, "the first failure");
        buffer.push(log::Level::Error, "the second failure");

        assert_eq!(buffer.entries().len(), 2);
        assert_eq!(buffer.error_count(), 2, "two failures are two failures");
    }

    /// The count must not become a second, unbounded kind of record: a loop that records one line
    /// forever leaves the buffer at a single entry of that line's own bytes, inside both caps.
    #[test]
    fn the_bounds_hold_when_every_record_is_identical() {
        let buffer = buffer();
        for _ in 0..MAX_RECORDS + 64 {
            buffer.push(log::Level::Error, "the same failure");
        }

        assert_eq!(
            buffer.len(),
            1,
            "identical records are one entry, not {MAX_RECORDS}"
        );
        assert_eq!(buffer.bytes(), "the same failure".len());
        assert!(buffer.bytes() <= MAX_TOTAL_BYTES);
        assert_eq!(
            buffer.entries()[0].repeats,
            (MAX_RECORDS + 64) as u32,
            "the count is how many records arrived"
        );
    }

    /// The caps are about text that is retained, so a repeated 2 KiB record is still one record's
    /// worth of bytes however many times it arrived.
    #[test]
    fn the_byte_cap_holds_when_every_record_is_identical() {
        let buffer = buffer();
        let record = "z".repeat(MAX_RECORD_BYTES);
        for _ in 0..100 {
            buffer.push(log::Level::Warn, &record);
        }

        assert_eq!(buffer.len(), 1);
        assert_eq!(
            buffer.bytes(),
            MAX_RECORD_BYTES,
            "one record's bytes, not a hundred"
        );
        assert!(buffer.bytes() <= MAX_TOTAL_BYTES);
        assert_eq!(buffer.entries()[0].repeats, 100);
    }

    /// A million repetitions must not become a page of digits: the count saturates at
    /// [`MAX_REPEATS`] and the report says so, so the report's size stops depending on how many
    /// times the line was recorded.
    #[test]
    fn a_huge_repeat_count_is_clamped_and_says_so() {
        let buffer = buffer();
        for _ in 0..MAX_REPEATS * 4 {
            buffer.push(log::Level::Error, "one stubborn failure");
        }

        let entries = buffer.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].repeats, MAX_REPEATS,
            "the count stops at the cap"
        );

        let report = buffer.report();
        assert!(
            report.contains("[ERROR] one stubborn failure [repeated 1000+ times]"),
            "{report}"
        );
        assert!(
            report.len() < 2 * MAX_RECORD_BYTES,
            "the report must not grow with the repetition count: {} bytes",
            report.len()
        );
    }

    #[test]
    fn nothing_below_error_level_counts_as_an_error() {
        let buffer = buffer();
        assert!(!buffer.has_errors(), "an empty buffer has no errors");
        assert_eq!(buffer.error_count(), 0);
        assert_eq!(buffer.latest_error(), None);

        for level in [log::Level::Trace, log::Level::Debug, log::Level::Info] {
            buffer.push(level, "not a failure");
        }
        buffer.push(log::Level::Warn, "still not a failure");
        assert!(!buffer.has_errors(), "only error level is an error");
        assert_eq!(buffer.error_count(), 0);

        buffer.push(log::Level::Error, "first failure");
        assert!(buffer.has_errors());
        assert_eq!(buffer.error_count(), 1);
        assert_eq!(buffer.latest_error().as_deref(), Some("first failure"));

        buffer.push(log::Level::Info, "afterwards");
        buffer.push(log::Level::Error, "second failure");
        assert_eq!(buffer.error_count(), 2);
        assert_eq!(
            buffer.latest_error().as_deref(),
            Some("second failure"),
            "the latest error, not the latest record"
        );
    }

    #[test]
    fn the_report_names_the_version_the_windows_and_the_tail() {
        let buffer = buffer();
        buffer.push(log::Level::Info, "alpha");
        buffer.push(log::Level::Error, "beta");

        let report = buffer.report();
        assert!(report.contains(VERSION), "{report}");
        assert!(
            report.contains(&crate::platform::os_version::description()),
            "the report must say which Windows this is: {report}"
        );
        assert!(report.contains("2 record(s) this session, 1 at error level."));
        assert!(report.contains("[ERROR] beta"), "{report}");
        assert!(report.contains("[INFO] alpha"), "{report}");
        assert!(
            report.find("alpha") < report.find("beta"),
            "the tail is oldest first: {report}"
        );
    }

    /// What a public issue receives: a collapsed entry is one legible line, and the report still
    /// carries the build, the Windows, the tail's own count, and both kinds of record.
    #[test]
    fn the_report_renders_a_collapsed_entry_and_keeps_every_fact() {
        let buffer = buffer();
        buffer.push(log::Level::Info, "alpha");
        buffer.push(log::Level::Error, "beta");
        buffer.push(log::Level::Error, "beta");

        let report = buffer.report();
        assert!(report.contains(VERSION), "{report}");
        assert!(
            report.contains(&crate::platform::os_version::description()),
            "the report must say which Windows this is: {report}"
        );
        assert!(
            report.contains("2 record(s) this session, 1 at error level."),
            "the two entries are what the tail below holds: {report}"
        );
        assert!(report.contains("[INFO] alpha"), "{report}");
        assert!(
            report.contains("[ERROR] beta [repeated 2 times]"),
            "{report}"
        );
        assert!(
            report.find("[INFO] alpha") < report.find("[ERROR] beta"),
            "the tail is still oldest first: {report}"
        );
        assert_eq!(
            report.lines().filter(|line| line.contains("beta")).count(),
            1,
            "the repeated line is printed once, not twice: {report}"
        );
    }

    /// The Discord webhook's token is its path, and the *host* is what identifies the line, so the
    /// host survives and the secret does not.
    #[test]
    fn a_webhook_url_loses_its_id_and_token_but_keeps_its_host() {
        let redacted = redact(
            "sending to https://discord.com/api/webhooks/12345678901234567/abcDEF-._~token now",
        );
        assert_eq!(
            redacted,
            "sending to https://discord.com/api/webhooks/<redacted> now"
        );

        // A versioned path, a bare path, and a canary host are all the same shape.
        assert_eq!(
            redact("https://canary.discord.com/api/v10/webhooks/123/tok"),
            "https://canary.discord.com/api/v10/webhooks/<redacted>"
        );
        assert_eq!(
            redact("/api/webhooks/1/t?x=1"),
            "/api/webhooks/<redacted>?x=1"
        );
    }

    #[test]
    fn an_authorization_header_loses_its_credential_and_nothing_else() {
        assert_eq!(
            redact("Authorization: Basic cmlvdDpzZWNyZXQ="),
            "Authorization: Basic <redacted>"
        );
        // Case is not what makes it a header, and the rest of the line is untouched.
        assert_eq!(
            redact("authorization: basic QWxhZGRpbjpvcGVuIHNlc2FtZQ== and more"),
            "authorization: basic <redacted> and more"
        );
        // The word "Basic" in prose, with no header introducing it, is left alone.
        assert_eq!(
            redact("Basic authentication failed"),
            "Basic authentication failed"
        );
        // A header whose value is not a Basic credential is left alone too.
        assert_eq!(
            redact("Authorization: Bearer abc.def.ghi"),
            "Authorization: Bearer abc.def.ghi"
        );
    }

    /// A formatted message can carry either separator, and the marker's case is not what makes it a
    /// profile path. Both are redacted the same way, and the marker is left as it was written.
    #[test]
    fn a_forward_slash_or_upper_case_profile_path_is_redacted_the_same_way() {
        assert_eq!(
            redact("path=C:/Users/Michael/AppData/Roaming/League Auto Accept/settings.json"),
            "path=C:/Users/<user>/AppData/Roaming/League Auto Accept/settings.json"
        );
        // Either separator, on either side of the name.
        assert_eq!(
            redact("path=C:\\Users/Michael/AppData/settings.json"),
            "path=C:\\Users/<user>/AppData/settings.json"
        );
        assert_eq!(
            redact("path=C:\\USERS\\Michael\\AppData\\settings.json"),
            "path=C:\\USERS\\<user>\\AppData\\settings.json"
        );
    }

    /// The account name is only replaced when it is a whole path component. A path that merely
    /// contains the letters is not a user profile.
    #[test]
    fn a_path_with_no_profile_component_is_untouched() {
        assert_eq!(
            redact("LCU at C:\\Riot Games\\League of Legends\\lockfile"),
            "LCU at C:\\Riot Games\\League of Legends\\lockfile"
        );
        assert_eq!(
            redact("C:\\PowerUsers\\Michael\\settings.json"),
            "C:\\PowerUsers\\Michael\\settings.json",
            "the word inside a longer segment is not the component"
        );
    }

    /// The narrowness doctrine in miniature: a record that merely mentions users must come back
    /// unchanged, or the redaction would be editing prose rather than protecting an account name.
    #[test]
    fn prose_that_merely_mentions_users_is_untouched() {
        assert_eq!(redact("allowed users: 5"), "allowed users: 5");
        assert_eq!(
            redact("sharing is enabled for all users/groups"),
            "sharing is enabled for all users/groups",
            "a separator after the word is not enough on its own"
        );
        assert_eq!(
            redact("the Users group has three members"),
            "the Users group has three members"
        );
    }

    /// The redaction is done on the way in, so even a caller that pushes the URL directly cannot get
    /// it into the report.
    #[test]
    fn a_redacted_record_is_what_the_report_contains() {
        const URL: &str =
            "https://discord.com/api/webhooks/22345678901234567/sentinel-token-aaaaaaaaaaaaaaaa";

        let buffer = buffer();
        buffer.push(log::Level::Error, URL);
        let report = buffer.report();

        assert!(!report.contains("sentinel-token"), "{report}");
        assert!(!report.contains("22345678901234567"), "{report}");
        assert!(report.contains(REDACTED), "{report}");
    }

    /// Redaction is applied on the way in, and the comparison is made on that redacted text, so two
    /// records that differ only in the secret are the same record. Nothing raw is compared, and
    /// nothing raw is retained: the entry holds the redacted form, and the report holds neither
    /// secret.
    #[test]
    fn records_that_differ_only_in_a_redacted_secret_collapse() {
        let first =
            "discord: https://discord.com/api/webhooks/12345678901234567/sentinel-token-one refused";
        let second =
            "discord: https://discord.com/api/webhooks/12345678901234567/sentinel-token-two refused";

        let buffer = buffer();
        buffer.push(log::Level::Error, first);
        buffer.push(log::Level::Error, second);

        let entries = buffer.entries();
        assert_eq!(entries.len(), 1, "their redacted forms are identical");
        assert_eq!(entries[0].repeats, 2);
        assert_eq!(
            entries[0].text, "discord: https://discord.com/api/webhooks/<redacted> refused",
            "the retained text is the redacted one"
        );

        let report = buffer.report();
        assert!(!report.contains("sentinel-token"), "{report}");
        assert!(!report.contains("12345678901234567"), "{report}");
        assert!(report.contains("[repeated 2 times]"), "{report}");
    }

    /// The record this rule exists for: a refused save names the settings file, and with it the
    /// account name. The name must go, and every fact a maintainer reads to tell the causes apart —
    /// the level, the module, the message, the integrity level and both write probes — must stay.
    #[test]
    fn a_refused_save_loses_the_account_name_and_keeps_every_fact() {
        const RECORD: &str = concat!(
            "league_auto_accept::config::store: settings save refused: Access is denied. (os error 5)",
            " | path=C:\\Users\\Michael\\AppData\\Roaming\\League Auto Accept\\settings.json",
            " | integrity=0x1000",
            " | can create file in directory=false",
            " | can open existing file for write=false",
            " | 432 bytes, readonly=false",
        );

        let buffer = buffer();
        buffer.push(log::Level::Error, RECORD);

        let entries = buffer.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, log::Level::Error, "the level is a fact");
        assert_eq!(
            entries[0].text,
            concat!(
                "league_auto_accept::config::store: settings save refused: Access is denied. (os error 5)",
                " | path=C:\\Users\\<user>\\AppData\\Roaming\\League Auto Accept\\settings.json",
                " | integrity=0x1000",
                " | can create file in directory=false",
                " | can open existing file for write=false",
                " | 432 bytes, readonly=false",
            ),
            "only the account name may change"
        );

        // The copied report is what a public issue receives, so the name must be absent from it in
        // full, not merely replaced in one field.
        let report = buffer.report();
        assert!(
            !report.contains("Michael"),
            "the account name must not be copied: {report}"
        );
        assert!(
            report.contains("[ERROR] league_auto_accept::config::store: settings save refused"),
            "the level and module survive: {report}"
        );
        assert!(
            report.contains("path=C:\\Users\\<user>\\AppData\\Roaming"),
            "the app-relative path survives: {report}"
        );
        assert!(
            report.contains("integrity=0x1000")
                && report.contains("can create file in directory=false")
                && report.contains("can open existing file for write=false"),
            "the integrity and both probe facts survive: {report}"
        );
    }

    /// A webhook URL is not the only thing that could be pasted: the redaction must not mangle text
    /// that is merely ordinary, or the report would be less useful than the failure.
    #[test]
    fn ordinary_records_are_left_alone() {
        assert_eq!(
            redact("settings file: C:\\ProgramData\\League Auto Accept\\settings.json"),
            "settings file: C:\\ProgramData\\League Auto Accept\\settings.json"
        );
        assert_eq!(redact("LCU at C:\\Riot Games"), "LCU at C:\\Riot Games");
        assert_eq!(redact(""), "");
    }

    /// Truncation must land on a character boundary even when the cap falls inside a multi-byte
    /// character, otherwise the record would not be a `String` at all.
    #[test]
    fn truncation_keeps_the_text_valid_wherever_the_cap_falls() {
        let text = "é".repeat(MAX_RECORD_BYTES);
        let truncated = truncate(&text);
        assert!(truncated.len() <= MAX_RECORD_BYTES, "{}", truncated.len());
        assert!(truncated.ends_with(TRUNCATION_MARKER));
        assert!(truncated.starts_with('é'));
    }
}
