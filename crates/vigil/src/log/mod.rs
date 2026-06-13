//! Logging substrate for vigil — Phase 5.2.
//!
//! Provides a tracing subscriber that writes log lines in the EXACT bash format:
//!
//!   YYYY-MM-DDTHH:MM:SS±HHMM LEVEL message
//!
//! Key points:
//! - Timestamp: local time with offset, second precision, `%z` = `±HHMM` (no colon).
//!   This matches bash `date '+%Y-%m-%dT%H:%M:%S%z'`.
//! - Single space between each field; no colon after timestamp; LEVEL uppercase.
//! - Tracing TRACE level maps to DEBUG (bash has no TRACE).
//! - Appender: NonBlocking over an APPEND-mode file. NEVER use rolling rotation
//!   (the appender's own rotator is not used; macOS relies on external newsyslog).
//! - A `LogRotation` seam exists; the macOS impl is a no-op stub in 5.2.
//! - EnvFilter defaults to "info"; overridable via RUST_LOG or VIGIL_LOG.

use std::fs::OpenOptions;
use std::io;

use tracing::Event;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

// ── Log rotation seam ─────────────────────────────────────────────────────────

/// Seam for log rotation. The macOS implementation (5.2) is a no-op stub;
/// rotation is managed externally by newsyslog which renames the file.
///
/// # macOS rotation note
/// newsyslog renames the current log file; the NonBlocking worker has the file
/// open by inode and will continue writing to the renamed inode until the
/// process reopens it. A `reload-log` reopen path is a later slice (Linux).
/// In 5.2 this seam exists; macOS relies on newsyslog and the file staying the
/// source of truth.
#[allow(dead_code)]
pub trait LogRotation {
    /// Ensure the rotation infrastructure is installed (newsyslog conf, etc.).
    /// In 5.2 this is a no-op; wired in 5.7.
    fn ensure_installed(&self) -> io::Result<()>;
}

/// macOS newsyslog rotation — no-op stub for 5.2.
#[allow(dead_code)]
pub struct MacOsNewsyslogRotation;

impl LogRotation for MacOsNewsyslogRotation {
    fn ensure_installed(&self) -> io::Result<()> {
        // 5.2 stub: newsyslog installation happens in 5.7 (cmd_setup).
        Ok(())
    }
}

// ── Custom FormatEvent — exact bash log-line format ───────────────────────────
//
// Bash format (lib/common.sh line 103–104):
//   ts=$(date '+%Y-%m-%dT%H:%M:%S%z')   # e.g. 2026-06-13T14:22:09+0530
//   line="$ts $level $*"
//
// So: YYYY-MM-DDTHH:MM:SS±HHMM LEVEL message\n
//   - %z with no colon, second precision, local offset
//   - single space between fields
//   - level uppercase (TRACE→DEBUG)

pub struct VigilLogFormat;

impl<S, N> FormatEvent<S, N> for VigilLogFormat
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        // Timestamp: local time, second precision, ±HHMM offset (no colon).
        // chrono's %z yields "+0530" (no colon), matching bash `date '%z'`.
        let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z");

        // Level: uppercase; TRACE → DEBUG (bash has no TRACE level).
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => "ERROR",
            tracing::Level::WARN => "WARN",
            tracing::Level::INFO => "INFO",
            tracing::Level::DEBUG => "DEBUG",
            tracing::Level::TRACE => "DEBUG",
        };

        // Emit: "<ts> <LEVEL> <message>"
        write!(writer, "{ts} {level} ")?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

// ── Subscriber init ───────────────────────────────────────────────────────────

/// Initialize the file-based tracing subscriber. Call once at process start;
/// hold the returned `WorkerGuard` alive for the entire process lifetime to
/// ensure all buffered log lines are flushed before exit.
///
/// `log_file_path`: absolute path to the log file. Opened in APPEND mode;
/// created if absent. The file is the cross-OS source of truth.
///
/// `log_level_override`: optional level string (e.g. "debug"). Falls back to
/// `RUST_LOG`, then `VIGIL_LOG`, then "info".
///
/// # Panics
/// Panics if the global subscriber has already been set (call once per process).
#[allow(dead_code)]
pub fn init_file_subscriber(
    log_file_path: &str,
    log_level_override: Option<&str>,
) -> io::Result<WorkerGuard> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file_path)?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    let filter = if let Some(level) = log_level_override {
        EnvFilter::new(level)
    } else {
        // Priority: RUST_LOG > VIGIL_LOG > "info"
        std::env::var("RUST_LOG")
            .or_else(|_| std::env::var("VIGIL_LOG"))
            .map(EnvFilter::new)
            .unwrap_or_else(|_| EnvFilter::new("info"))
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(VigilLogFormat)
                .with_writer(non_blocking)
                .with_ansi(false),
        )
        .init();

    Ok(guard)
}
