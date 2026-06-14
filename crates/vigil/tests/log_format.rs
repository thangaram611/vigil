//! Integration test for the bash-format log line.
//!
//! Runs in its own integration-test process to get a fresh global-subscriber
//! state (avoiding the "subscriber already set" panic if the unit tests also
//! try to set a global subscriber).
//!
//! Asserts that a `tracing::info!("hello world")` line emitted through
//! `init_file_subscriber` matches the exact bash log format:
//!
//!   YYYY-MM-DDTHH:MM:SS±HHMM INFO hello world
//!
//! Regex: `^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}[+-][0-9]{4} (INFO|WARN|ERROR|DEBUG) .*$`

use std::fs;

fn vigil_log_regex() -> regex::Regex {
    regex::Regex::new(
        r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}[+-][0-9]{4} (INFO|WARN|ERROR|DEBUG) .+$",
    )
    .unwrap()
}

/// Shared read+assert tail: read the first line of `log_path`, assert it matches
/// the bash-format regex, then assert it `contains` the given level/message
/// substring. This ONLY reads the file and asserts — it never sets up or scopes
/// a tracing subscriber, so each caller keeps full control of its own subscriber
/// lifecycle (global `init_file_subscriber` vs scoped `with_default`).
fn assert_first_line(log_path: &std::path::Path, substr: &str) -> String {
    let content = fs::read_to_string(log_path).expect("read log file");
    let line = content
        .lines()
        .next()
        .expect("log file must have at least one line")
        .to_string();
    let re = vigil_log_regex();
    assert!(
        re.is_match(&line),
        "log line does not match bash format regex.\nLine: {line:?}\nRegex: {re}"
    );
    assert!(
        line.contains(substr),
        "log line must contain {substr:?}.\nLine: {line:?}"
    );
    line
}

#[test]
fn log_line_matches_bash_format() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let log_path = tmp.path().join("test.log");

    // Init the subscriber against the temp file.
    let guard = vigil::log::init_file_subscriber(log_path.to_str().unwrap(), Some("info"))
        .expect("init subscriber");

    // Emit a line.
    tracing::info!("hello world");

    // Flush: drop the guard to ensure NonBlocking flushes before we read.
    drop(guard);

    // Shared read + regex-format + level/message-substring tail.
    let line = assert_first_line(&log_path, " INFO hello world");

    // Stronger than the shared `contains`: the line must END with the exact
    // message (no target, no span).
    assert!(
        line.ends_with(" INFO hello world"),
        "log line must end with ' INFO hello world'.\nLine: {line:?}"
    );
}

#[test]
fn log_line_warn_level() {
    // Spin up a second subscriber in its own tmp file via a separate test
    // binary invocation — but we CAN'T set two global subscribers. Instead,
    // use `tracing::subscriber::with_default` for a scoped test.
    let tmp = tempfile::tempdir().expect("tempdir");
    let log_path = tmp.path().join("warn.log");

    use std::fs::OpenOptions;
    use tracing_appender::non_blocking;
    use tracing_subscriber::layer::SubscriberExt;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("open log file");
    let (non_blocking_writer, guard) = non_blocking(file);

    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .event_format(vigil::log::VigilLogFormat)
            .with_writer(non_blocking_writer)
            .with_ansi(false),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!("test warning");
    });

    drop(guard);

    // Shared read + regex-format + level-substring tail. The with_default scope
    // above is this test's own concern; the helper only reads + asserts.
    assert_first_line(&log_path, " WARN ");
}
