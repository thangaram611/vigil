//! `vigil log [-f|--follow]` — view / tail the daemon log (Phase 5.7 §4.11,
//! Contract 3 §2).
//!
//! Read-only: does NOT call `ensure_state_dir`. Behaviour:
//!   - `$1` exactly `-f`/`--follow` → follow (`tail -f` semantics, unbounded
//!     stream); anything else is ignored (no error).
//!   - Missing log → print `no log yet at {log_file}` to STDOUT, return 0 (NOT an
//!     error).
//!   - No-follow → PAGING / line-limit: print only the last `LINE_LIMIT` lines.
//!     This is the ONE intentional deviation from the bash `cat` (which dumped the
//!     whole file): the Rust port MUST NOT emit a megabyte dump (§4.11).

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use super::load_config_or_exit;

/// The no-follow line cap. The bash `cat`-the-whole-file behaviour is the thing we
/// are deliberately NOT reproducing; the daemon log is rotated by newsyslog, but a
/// single pre-rotation segment can still be large, so we show the most recent
/// `LINE_LIMIT` lines (the operationally useful tail) and a hint when truncated.
const LINE_LIMIT: usize = 2000;

/// Entry point for the `Log` dispatch arm. Returns `!` (always exits 0 on the
/// happy path; only an unreadable-log IO error exits non-zero).
pub fn run(args: Vec<OsString>) -> ! {
    let follow = matches!(
        args.first().and_then(|a| a.to_str()),
        Some("-f") | Some("--follow")
    );

    let cfg = load_config_or_exit();
    let log_file = cfg.log_file.clone();
    let path = Path::new(&log_file);

    // Missing log → soft message to STDOUT, exit 0 (bash: `echo ... ; return 0`).
    if !path.is_file() {
        // Plain println (not anstream-styled): this is informational, matches the
        // bash literal exactly so any consumer/test sees the same bytes.
        println!("no log yet at {log_file}");
        std::process::exit(0);
    }

    if follow {
        follow_log(path);
    } else {
        print_tail(path);
    }
    std::process::exit(0);
}

/// Print the last `LINE_LIMIT` lines of the log (the intentional paging deviation
/// from bash `cat`). Emits a leading truncation hint to STDERR when the file had
/// more lines, so STDOUT stays clean log content.
fn print_tail(path: &Path) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            anstream::eprintln!("vigil: log: {}: {e}", path.display());
            std::process::exit(super::EX_ERROR);
        }
    };
    // Ring buffer of the last LINE_LIMIT lines. We never hold the whole file in
    // memory beyond the window, so a multi-megabyte log is bounded.
    let mut window: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut total: usize = 0;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            // A non-UTF-8 / unreadable line ends the read gracefully (bash `cat`
            // would emit raw bytes; for the tail view we stop at the read error).
            Err(_) => break,
        };
        total += 1;
        window.push_back(line);
        if window.len() > LINE_LIMIT {
            window.pop_front();
        }
    }

    if total > window.len() {
        anstream::eprintln!(
            "vigil: log: showing the last {} of {} lines (use 'vigil log -f' to follow)",
            window.len(),
            total
        );
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in &window {
        // Plain bytes; let a closed pipe (e.g. `| head`) end the loop quietly.
        if writeln!(out, "{line}").is_err() {
            break;
        }
    }
    let _ = out.flush();
}

/// Follow the log (`tail -f` semantics): print existing content from a bounded
/// tail, then poll for appended bytes and stream them. Runs until interrupted
/// (SIGINT/SIGPIPE) — unbounded by design, like `tail -f`.
fn follow_log(path: &Path) -> ! {
    // First, emit the existing tail (bounded), matching `tail -f` which by default
    // shows the last lines before following.
    print_tail(path);

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            anstream::eprintln!("vigil: log: {}: {e}", path.display());
            std::process::exit(super::EX_ERROR);
        }
    };
    // Seek to EOF and stream appended bytes.
    let mut pos = file.seek(SeekFrom::End(0)).unwrap_or(0);
    let stdout = std::io::stdout();
    let mut buf = [0u8; 8192];
    loop {
        match file.read(&mut buf) {
            Ok(0) => {
                // No new data; the file may also have been truncated/rotated by
                // newsyslog. Detect a shrink and re-seek to its start.
                if std::fs::metadata(path)
                    .map(|m| m.len() < pos)
                    .unwrap_or(false)
                {
                    pos = file.seek(SeekFrom::Start(0)).unwrap_or(0);
                    continue;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Ok(n) => {
                pos += n as u64;
                let mut out = stdout.lock();
                if out.write_all(&buf[..n]).is_err() || out.flush().is_err() {
                    // Downstream pipe closed (e.g. `| head`); stop cleanly.
                    std::process::exit(0);
                }
            }
            Err(e) => {
                anstream::eprintln!("vigil: log: {}: {e}", path.display());
                std::process::exit(super::EX_ERROR);
            }
        }
    }
}
