//! Caffeinate assertion seam — Phase 5.5.
//!
//! Vigil holds user-idle system sleep with `caffeinate -i` (NO display
//! assertion, so native lock + display sleep remain undisturbed). The liveness
//! check is BY IDENTITY (not a bare `kill -0`): a pid counts as our live
//! assertion ONLY if it is alive AND its `ps` command basename is `caffeinate`
//! AND it is NOT a display-holding caffeinate.
//!
//! The stale-display predicate is the PURE [`is_stale_display`] fn so it is
//! unit-testable without spawning. The trait impl reads the `ps` argv and
//! delegates to it.

/// PURE stale-display predicate over a caffeinate argv string.
///
/// Ports the bash regex `(^|[[:space:]])-[A-Za-z]*d[A-Za-z]*($|[[:space:]])`:
/// a `-`-prefixed flag cluster containing a LITERAL LOWERCASE `d` (the display
/// assertion) anywhere in its letters. So BOTH `-di` and `-dimsu` are stale
/// (they hold the display), while `-i` alone is NOT stale. Uppercase `-D` is NOT
/// stale (the bash regex `d` is lowercase only, and `caffeinate` has no `-D`
/// flag — faithful parity over a defensive super-set).
///
/// Older Vigil used `caffeinate -di`, which held a display assertion. Treat any
/// display-holding caffeinate as stale so the next reconcile replaces it with a
/// fresh `-i`.
pub fn is_stale_display(argv: &str) -> bool {
    // Scan whitespace-delimited tokens; a token of the form `-<letters>` whose
    // letters include `d` is a display-holding flag cluster.
    for token in argv.split_whitespace() {
        if let Some(flags) = token.strip_prefix('-') {
            // The bash regex requires the cluster to be `[A-Za-z]*d[A-Za-z]*`,
            // i.e. ALL letters and containing a LITERAL LOWERCASE `d`. A token
            // like `-d=foo` would not match the all-letters cluster; mirror that
            // by requiring the whole flags run to be ASCII-alphabetic.
            if !flags.is_empty()
                && flags.bytes().all(|b| b.is_ascii_alphabetic())
                && flags.bytes().any(|b| b == b'd')
            {
                return true;
            }
        }
    }
    false
}

/// Caffeinate assertion seam. The real impl spawns `caffeinate -i`; a fake drives
/// unit tests without spawning.
pub trait CaffeinateAssertion {
    /// Spawn a fresh `caffeinate -i` and return its pid.
    fn spawn(&self) -> std::io::Result<u32>;
    /// True iff `pid` is alive AND is a non-display-holding `caffeinate` (the
    /// alive-BY-IDENTITY check).
    fn is_alive_by_identity(&self, pid: u32) -> bool;
    /// True iff `pid` is alive AND its `ps` argv basename is `caffeinate`
    /// (REGARDLESS of the stale-display gate). Mirrors the bash
    /// `vigil_pmset_spawn_caffeinate` guard `[[ "$old_base" == "caffeinate" ]]`:
    /// the OLD pid may be a stale display-holding caffeinate (alive but NOT
    /// alive-by-identity) — bash still kills it — but a pid the OS has recycled
    /// onto an unrelated non-caffeinate process must NOT be killed. This is the
    /// predicate `spawn_caffeinate` uses to gate the replacement kill.
    fn is_caffeinate_basename(&self, pid: u32) -> bool;
    /// Send SIGTERM to `pid` (best-effort). Used to replace a stale assertion.
    fn kill(&self, pid: u32);
}

/// Real macOS caffeinate. Spawns `caffeinate -i` (no display hold) via
/// `std::process::Command`.
pub struct MacCaffeinate;

impl MacCaffeinate {
    /// Read the `ps -p <pid> -o command=` argv for `pid`, trimmed. None if the
    /// pid is not found / ps failed.
    fn ps_command(pid: u32) -> Option<String> {
        let out = std::process::Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    /// Basename of the first whitespace-delimited token of an argv string.
    fn argv_basename(argv: &str) -> &str {
        let exe = argv.split_whitespace().next().unwrap_or("");
        exe.rsplit('/').next().unwrap_or(exe)
    }

    /// Apply the alive-by-identity rule to a known argv string. Pure given the
    /// argv (the IO is the `ps` read in [`Self::ps_command`]).
    fn identity_ok(argv: &str) -> bool {
        Self::argv_basename(argv) == "caffeinate" && !is_stale_display(argv)
    }
}

impl CaffeinateAssertion for MacCaffeinate {
    fn spawn(&self) -> std::io::Result<u32> {
        let child = std::process::Command::new("/usr/bin/caffeinate")
            .arg("-i")
            .spawn()?;
        Ok(child.id())
    }

    fn is_alive_by_identity(&self, pid: u32) -> bool {
        // kill(pid, 0) liveness, then identity via ps argv.
        // SAFETY: kill with signal 0 only probes for existence; no memory ops.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        if !alive {
            return false;
        }
        match Self::ps_command(pid) {
            Some(argv) => Self::identity_ok(&argv),
            None => false,
        }
    }

    fn is_caffeinate_basename(&self, pid: u32) -> bool {
        // kill(pid, 0) liveness, then basename via ps argv (NO stale-display
        // gate — bash kills a stale display-holding caffeinate too).
        // SAFETY: kill with signal 0 only probes for existence; no memory ops.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        if !alive {
            return false;
        }
        match Self::ps_command(pid) {
            Some(argv) => Self::argv_basename(&argv) == "caffeinate",
            None => false,
        }
    }

    fn kill(&self, pid: u32) {
        // SAFETY: SIGTERM to a pid; no invariants. Best-effort.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_display_matches_bash_regex() {
        // -i alone is NOT stale (no `d`).
        assert!(!is_stale_display("caffeinate -i"));
        assert!(!is_stale_display("/usr/bin/caffeinate -i"));
        // -di IS stale.
        assert!(is_stale_display("caffeinate -di"));
        // -dimsu IS stale (the full old display+others cluster).
        assert!(is_stale_display("caffeinate -dimsu"));
        // leading/trailing spaces around the token.
        assert!(is_stale_display("  caffeinate   -dimsu  "));
        // -s alone is NOT stale.
        assert!(!is_stale_display("caffeinate -s"));
        // a cluster with d in the middle.
        assert!(is_stale_display("caffeinate -ides"));
        // no flags at all.
        assert!(!is_stale_display("caffeinate"));
        // uppercase -D is NOT a display hold: the bash regex `d` is lowercase
        // only, and caffeinate has no `-D` flag. Faithful parity with bash.
        assert!(!is_stale_display("caffeinate -D"));
    }

    #[test]
    fn argv_basename_extraction() {
        assert_eq!(
            MacCaffeinate::argv_basename("/usr/bin/caffeinate -i"),
            "caffeinate"
        );
        assert_eq!(MacCaffeinate::argv_basename("caffeinate -i"), "caffeinate");
        assert_eq!(MacCaffeinate::argv_basename("/bin/sleep 60"), "sleep");
    }

    #[test]
    fn identity_ok_requires_caffeinate_basename_and_not_stale() {
        assert!(MacCaffeinate::identity_ok("/usr/bin/caffeinate -i"));
        // right basename but display-holding => not ok (stale).
        assert!(!MacCaffeinate::identity_ok("caffeinate -di"));
        // wrong basename even with -i => not ok.
        assert!(!MacCaffeinate::identity_ok("/bin/sleep -i"));
    }
}
