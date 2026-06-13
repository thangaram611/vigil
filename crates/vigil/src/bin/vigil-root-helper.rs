//! vigil-root-helper — the privileged power-setting transition helper (Phase 5.5).
//!
//! Thin `main()` ONLY. All logic lives in `vigil::helper` / `vigil::power` so it
//! is cfg(test)-unit-testable. This binary:
//!   1. parses argv into a `HelperConfig` (validated; `--allowed-uid` baked in),
//!   2. enforces the non-root refusal gate (`require_root`),
//!   3. runs `--once` (one poll pass) or `--serve` (poll loop).
//!
//! The root-refusal check and the install-time-fixed allowed-uid come ONLY from
//! validated argv — NEVER from request content.
//!
//! ## Compile-time test seam
//! When built WITHOUT `--features helper-test-seam`, the shipped binary uses the
//! REAL `MacPmset` / `MacSleepReader` and `require_root()` enforces root. The
//! `helper-test-seam` feature swaps in the file-backed fakes and the non-root
//! bypass (for the subprocess adversarial test). `VIGIL_ROOT_HELPER_TESTING=1`
//! in the env is NEVER consulted — the red-team test proves it cannot flip the
//! seam.

use std::process::ExitCode;

use vigil::helper::{self, HelperConfig};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match helper::parse_args(args) {
        Ok(c) => c,
        Err(helper::ArgError::Help) => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("vigil-root-helper: {e}");
            // Bash used exit 64 for usage and exit 1 for a die(); a missing/bad
            // arg is a usage error.
            return ExitCode::from(64);
        }
    };

    if let Err(e) = helper::require_root() {
        eprintln!("vigil-root-helper: {e}");
        return ExitCode::from(1);
    }

    if cfg.once {
        match run_once(&cfg) {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("vigil-root-helper: {e}");
                ExitCode::from(1)
            }
        }
    } else {
        // --serve: poll forever. Validation failures inside the loop are logged
        // and retried on the next tick (matching bash's `|| true` resilience).
        loop {
            let _ = run_once(&cfg);
            std::thread::sleep(std::time::Duration::from_secs(cfg.poll_secs));
        }
    }
}

/// Run one poll pass with the appropriate seams (real vs feature-gated fakes).
fn run_once(cfg: &HelperConfig) -> Result<usize, String> {
    #[cfg(feature = "helper-test-seam")]
    {
        use vigil::power::pmset::fake::{FakePmset, FakeSleepReader};
        helper::process_once_with_seams(cfg, &FakePmset, &FakeSleepReader)
    }
    #[cfg(not(feature = "helper-test-seam"))]
    {
        use vigil::power::pmset::{MacPmset, MacSleepReader};
        helper::process_once_with_seams(cfg, &MacPmset, &MacSleepReader)
    }
}

fn print_usage() {
    eprintln!(
        "usage: vigil-root-helper --serve --request-dir DIR --response-dir DIR \
         --state-dir DIR --log-file FILE --allowed-uid UID --allowed-user USER\n\
         \x20      vigil-root-helper --once  --request-dir DIR --response-dir DIR \
         --state-dir DIR --log-file FILE --allowed-uid UID --allowed-user USER"
    );
}
