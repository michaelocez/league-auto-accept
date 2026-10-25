//! In-memory diagnostics: the recent records of this session, kept so a failure in the field can be
//! read at all.
//!
//! The shipped release is a GUI-subsystem binary whose stderr does not exist, so a `log::error!` in
//! the field is invisible. The owner's decision is that this application creates **no** log file:
//! instead the last records live in a bounded buffer, and Settings shows them only once something at
//! error level has actually been recorded, with a button that copies a report for a GitHub issue.
//!
//! Two properties hold that arrangement together:
//!
//! * Nothing is written anywhere but the buffer. A debug build still forwards every record to
//!   `env_logger`'s stderr exactly as it always has; a release build deliberately stops at the
//!   buffer, and the buffer dies with the process.
//! * Every record is redacted on the way in, so the report cannot carry the Discord webhook URL
//!   (whose path *is* the token), an LCU `Authorization` credential, or anything else listed in
//!   [`redact`] — however the text reached the logger.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::app::update::VERSION;

/// How many records are kept at most.
pub const MAX_RECORDS: usize = 256;
/// How many bytes of record text are kept at most, whichever comes first with [`MAX_RECORDS`].
pub const MAX_TOTAL_BYTES: usize = 32 * 1024;
/// The longest single record kept, so one enormous record cannot flush the whole buffer.
pub const MAX_RECORD_BYTES: usize = 2048;

/// What the end of a truncated record says.
const TRUNCATION_MARKER: &str = "… (truncated)";
/// What a redacted value is replaced with.
const REDACTED: &str = "<redacted>";

/// One retained record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The level the record was made at, so the buffer can answer "did anything fail?".
    pub level: log::Level,
    /// The record's module and message, already redacted and already bounded.
    pub text: String,
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
    /// Retains one record, redacted and bounded.
    ///
    /// Redaction and truncation happen here rather than at either end, so no caller can put text
    /// into the buffer that came from somewhere else: the logger is not the only possible writer.
    pub fn push(&self, level: log::Level, text: &str) {
        let text = truncate(&redact(text));
        let mut inner = self.lock();
        inner.bytes += text.len();
        inner.records.push_back(Entry { level, text });
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
                .map(|entry| format!("[{}] {}", entry.level, entry.text)),
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

/// Every secret shape that must not survive into the buffer.
///
/// Two, and deliberately only two — a report that redacted indiscriminately would hide the facts a
/// maintainer needs:
///
/// * a Discord webhook URL, whose *path* is the secret: `/api/webhooks/<id>/<token>`. The host and
///   the `/webhooks/` marker are kept, because they are what identifies the line.
/// * an HTTP `Authorization: Basic …` value, which is the LCU credential.
pub fn redact(text: &str) -> String {
    // Per line, because an `Authorization` header's value ends where its line does.
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&redact_authorization_value(&redact_runs_after(
            line,
            "webhooks/",
            is_webhook_path_character,
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
    #[test]
    fn the_byte_cap_bounds_the_buffer_before_the_record_count_does() {
        let buffer = buffer();
        let record = "y".repeat(MAX_RECORD_BYTES);
        for _ in 0..20 {
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

    /// A webhook URL is not the only thing that could be pasted: the redaction must not mangle text
    /// that is merely ordinary, or the report would be less useful than the failure.
    #[test]
    fn ordinary_records_are_left_alone() {
        assert_eq!(
            redact("settings file: C:\\Users\\Michael\\settings.json"),
            "settings file: C:\\Users\\Michael\\settings.json"
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
