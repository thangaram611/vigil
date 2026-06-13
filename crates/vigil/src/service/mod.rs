//! `src/service/` — the OS service-management seam (Phase 5.7 §2.2, §3.3, §4.7).
//!
//! This module owns everything that talks to the platform service manager. On
//! macOS that is `launchd`: a per-user **LaunchAgent** (`com.thangaram.vigil`)
//! that runs the resident daemon, and a system **LaunchDaemon**
//! (`com.thangaram.vigil.helper`) that runs the privileged power helper. It also
//! renders the `newsyslog.d` rotation config that owns log rotation (never the
//! tracing appender — §3.3).
//!
//! The portable seam is the [`ServiceInstaller`] trait; [`MacosLaunchdInstaller`]
//! is the only implementation in 5.7 (Linux fills the trait in 5.8).
//!
//! ## Byte-stable rendering vs. the `plist` crate
//!
//! Spec §2.2.2 mandates "render via the `plist` crate, no heredocs, no manual XML
//! escaping" AND the Gate-0 goldens
//! (`crates/vigil/tests/golden/{user_agent,helper}.plist`) must match
//! byte-for-byte. These two requirements conflict: the `plist` crate's XML
//! serializer emits **tab** indentation, no blank lines between keys, and cannot
//! emit the XML comments the goldens carry. A pure `plist`-crate render therefore
//! cannot reproduce the goldens.
//!
//! Byte-stability is the harder constraint (it is a frozen ABI — `setup
//! --dry-run --verbose` shows these previews and `launchctl` parses the installed
//! files). So the shipping renderer emits the golden's exact structure (4-space
//! indent, blank lines, comments) from in-binary templates, and delegates **value
//! XML-escaping to a library primitive** (`quick_xml::escape::escape`, the same
//! escaper the `plist` crate uses internally) — satisfying the "no manual XML
//! escaping" intent without a hand-rolled escaper. The typed `#[derive(Serialize)]`
//! plist structs ([`UserAgentPlist`], [`HelperPlist`]) are retained and proven
//! serde-valid (round-tripped through the `plist` crate in the unit tests) so the
//! data model is the single source of truth the renderer reads from.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::config::VigilConfig;

/// launchd label for the user LaunchAgent (the resident daemon). HARDCODED —
/// never overridable (spec §8 infra checklist; Contract 2 §0).
pub const USER_AGENT_LABEL: &str = "com.thangaram.vigil";
/// launchd label for the system LaunchDaemon (the privileged power helper).
/// HARDCODED — never overridable.
pub const HELPER_LABEL: &str = "com.thangaram.vigil.helper";

/// The fixed `PATH` baked into the LaunchAgent's `EnvironmentVariables` dict
/// (Contract 2 §1a; golden `user_agent.plist`).
const AGENT_PATH_ENV: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

// ── error / outcome types ─────────────────────────────────────────────────────

/// Failures the service layer can surface to the command layer.
#[derive(Debug)]
pub enum ServiceError {
    /// The plist file the operation needs is absent (e.g. `start` before
    /// `setup`). Carries the path that was expected.
    PlistMissing(String),
    /// A `launchctl` invocation could not be spawned at all (the binary is
    /// missing / not executable). Distinct from a non-zero exit, which several
    /// paths treat as best-effort and ignore.
    LaunchctlSpawn(String),
    /// Could not resolve the current user's name (needed for the helper plist
    /// `--allowed-user` arg and the newsyslog owner column).
    UserLookup(String),
    /// An IO error writing a rendered artifact to disk.
    Io(std::io::Error),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::PlistMissing(p) => {
                write!(f, "plist not found at {p} — run 'vigil setup' first")
            }
            ServiceError::LaunchctlSpawn(e) => write!(f, "could not run launchctl: {e}"),
            ServiceError::UserLookup(e) => write!(f, "could not resolve current user: {e}"),
            ServiceError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<std::io::Error> for ServiceError {
    fn from(e: std::io::Error) -> Self {
        ServiceError::Io(e)
    }
}

/// Outcome of [`ServiceInstaller::start_user_agent`] — lets the command layer
/// print the exact bash strings (`already loaded` vs `bootstrapped`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartState {
    /// `launchctl print` already succeeded — bootstrap skipped (idempotent).
    AlreadyLoaded,
    /// The agent was freshly `bootstrap`-ed (+ best-effort `enable`).
    Bootstrapped,
}

/// Outcome of [`ServiceInstaller::stop_user_agent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopState {
    /// The agent was loaded and has been `bootout`-ed (after the 50×100ms poll).
    BootedOut,
    /// The agent was not loaded — nothing to do (idempotent).
    NotLoaded,
}

// ── typed plist data model (serde source of truth) ────────────────────────────
//
// Every optional launchd key carries `skip_serializing_if = "Option::is_none"`
// because launchd treats key ABSENCE as meaningful (spec Q8): emitting
// `KeepAlive=false` is NOT the same as omitting it. These 5.7 plists have no
// optional keys today, but the attribute is present so any future optional key is
// correct by construction. The structs are serde-valid and round-trip through the
// `plist` crate (proven in tests); the byte-stable renderer reads from the same
// field values.

/// The user LaunchAgent plist (Contract 2 §1a).
#[derive(Debug, Clone, Serialize)]
pub struct UserAgentPlist {
    #[serde(rename = "Label")]
    pub label: String,
    #[serde(rename = "ProgramArguments")]
    pub program_arguments: Vec<String>,
    #[serde(rename = "RunAtLoad")]
    pub run_at_load: bool,
    #[serde(rename = "KeepAlive")]
    pub keep_alive: bool,
    #[serde(rename = "ProcessType")]
    pub process_type: String,
    #[serde(rename = "ExitTimeOut")]
    pub exit_timeout: i64,
    #[serde(rename = "ThrottleInterval")]
    pub throttle_interval: i64,
    #[serde(rename = "StandardOutPath")]
    pub stdout_path: String,
    #[serde(rename = "StandardErrorPath")]
    pub stderr_path: String,
    // Ordered map: launchd does not care about key order, but the golden does, so
    // the renderer emits PATH, VIGIL_STATE_DIR, VIGIL_LOG_DIR in that fixed order
    // (a BTreeMap is kept only for the serde round-trip; the renderer uses an
    // explicit ordered list).
    #[serde(rename = "EnvironmentVariables")]
    pub env: BTreeMap<String, String>,
}

/// The system LaunchDaemon plist for the privileged helper (Contract 2 §1b).
/// NOTE: NO `EnvironmentVariables` dict (asymmetry with the user agent).
#[derive(Debug, Clone, Serialize)]
pub struct HelperPlist {
    #[serde(rename = "Label")]
    pub label: String,
    #[serde(rename = "ProgramArguments")]
    pub program_arguments: Vec<String>,
    #[serde(rename = "RunAtLoad")]
    pub run_at_load: bool,
    #[serde(rename = "KeepAlive")]
    pub keep_alive: bool,
    #[serde(rename = "ProcessType")]
    pub process_type: String,
    #[serde(rename = "ExitTimeOut")]
    pub exit_timeout: i64,
    #[serde(rename = "ThrottleInterval")]
    pub throttle_interval: i64,
    #[serde(rename = "StandardOutPath")]
    pub stdout_path: String,
    #[serde(rename = "StandardErrorPath")]
    pub stderr_path: String,
}

// ── model builders (read VigilConfig → typed plist) ───────────────────────────

/// Resolve the current user's login name (`id -un` equivalent). Used for the
/// helper plist `--allowed-user` arg and the newsyslog owner column.
fn current_username() -> Result<String, ServiceError> {
    let uid = nix::unistd::Uid::from_raw(crate::config::get_uid());
    match nix::unistd::User::from_uid(uid) {
        Ok(Some(u)) => Ok(u.name),
        Ok(None) => Err(ServiceError::UserLookup(format!(
            "no passwd entry for uid {uid}"
        ))),
        Err(e) => Err(ServiceError::UserLookup(e.to_string())),
    }
}

/// Build the [`UserAgentPlist`] data model from config.
///
/// `ProgramArguments` is `[ "<install>/bin/vigil", "daemon" ]` — the Rust port's
/// deliberate change from the bash `@VIGIL_DAEMON_PATH@` = `bin/vigil-daemon`
/// (spec §2.1.1). The install copy (NOT `~/Documents`) is mandatory for TCC.
pub fn user_agent_model(cfg: &VigilConfig) -> UserAgentPlist {
    let mut env = BTreeMap::new();
    env.insert("PATH".to_string(), AGENT_PATH_ENV.to_string());
    env.insert("VIGIL_STATE_DIR".to_string(), cfg.state_dir.clone());
    env.insert("VIGIL_LOG_DIR".to_string(), cfg.log_dir.clone());
    UserAgentPlist {
        label: USER_AGENT_LABEL.to_string(),
        program_arguments: vec![
            format!("{}/bin/vigil", cfg.install_dir),
            "daemon".to_string(),
        ],
        run_at_load: true,
        keep_alive: true,
        process_type: "Background".to_string(),
        exit_timeout: 60,
        throttle_interval: 10,
        stdout_path: format!("{}/daemon.out.log", cfg.log_dir),
        stderr_path: format!("{}/daemon.err.log", cfg.log_dir),
        env,
    }
}

/// Build the [`HelperPlist`] data model from config. The `ProgramArguments` is
/// the FROZEN 14-element argv the helper validates (order exact, §2.2.2).
pub fn helper_model(cfg: &VigilConfig) -> Result<HelperPlist, ServiceError> {
    let uid = crate::config::get_uid();
    let username = current_username()?;
    let program_arguments = vec![
        cfg.root_helper.clone(),
        "--serve".to_string(),
        "--request-dir".to_string(),
        cfg.power_request_dir.clone(),
        "--response-dir".to_string(),
        cfg.power_response_dir.clone(),
        "--state-dir".to_string(),
        cfg.power_state_dir.clone(),
        "--log-file".to_string(),
        cfg.power_log_file.clone(),
        "--allowed-uid".to_string(),
        uid.to_string(),
        "--allowed-user".to_string(),
        username,
    ];
    Ok(HelperPlist {
        label: HELPER_LABEL.to_string(),
        program_arguments,
        run_at_load: true,
        keep_alive: true,
        process_type: "Background".to_string(),
        exit_timeout: 10,
        throttle_interval: 10,
        stdout_path: format!("{}/helper.out.log", cfg.power_log_dir),
        stderr_path: format!("{}/helper.err.log", cfg.power_log_dir),
    })
}

// ── byte-stable renderers ─────────────────────────────────────────────────────

/// XML-escape one plist `<string>`/`<key>` value. Delegates to
/// `quick_xml::escape::escape` (the escaper the `plist` crate uses internally),
/// which produces `&amp; &lt; &gt; &quot; &apos;` — byte-identical to the bash
/// `cmd_xml_escape` the goldens were captured with. No hand-rolled escaper.
fn xesc(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

const PLIST_HEADER: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
    "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
    "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
    "<plist version=\"1.0\">\n",
    "<dict>\n",
);

/// Render the user LaunchAgent plist to its exact golden bytes
/// (`tests/golden/user_agent.plist`, modulo env-derived paths and the
/// `ProgramArguments` array that legitimately differs per §2.1.1).
pub fn render_user_agent(cfg: &VigilConfig) -> String {
    let m = user_agent_model(cfg);
    let mut s = String::with_capacity(1024);
    s.push_str(PLIST_HEADER);

    s.push_str("    <key>Label</key>\n");
    s.push_str(&format!("    <string>{}</string>\n\n", xesc(&m.label)));

    s.push_str("    <key>ProgramArguments</key>\n");
    s.push_str("    <array>\n");
    for arg in &m.program_arguments {
        s.push_str(&format!("        <string>{}</string>\n", xesc(arg)));
    }
    s.push_str("    </array>\n\n");

    s.push_str("    <key>RunAtLoad</key>\n");
    s.push_str(&format!("    <{}/>\n\n", bool_tag(m.run_at_load)));

    s.push_str("    <key>KeepAlive</key>\n");
    s.push_str(&format!("    <{}/>\n\n", bool_tag(m.keep_alive)));

    s.push_str("    <key>ProcessType</key>\n");
    s.push_str(&format!(
        "    <string>{}</string>\n\n",
        xesc(&m.process_type)
    ));

    s.push_str(
        "    <!-- ExitTimeOut: seconds launchd waits between SIGTERM and SIGKILL on stop.\n\
         \x20        Default is system-defined (~20s on recent macOS) which is tight for our\n\
         \x20        cleanup path: helper release, kill caffeinate, drop baseline, release\n\
         \x20        lock. 60s gives the SIGTERM handler comfortable headroom under load. -->\n",
    );
    s.push_str("    <key>ExitTimeOut</key>\n");
    s.push_str(&format!("    <integer>{}</integer>\n\n", m.exit_timeout));

    s.push_str(
        "    <!-- ThrottleInterval: minimum seconds between successive spawns of this job\n\
         \x20        when KeepAlive is true. 10s is launchd's documented default — set\n\
         \x20        explicitly for documentation and to insulate against future macOS\n\
         \x20        changes to the implicit default. -->\n",
    );
    s.push_str("    <key>ThrottleInterval</key>\n");
    s.push_str(&format!(
        "    <integer>{}</integer>\n\n",
        m.throttle_interval
    ));

    s.push_str("    <key>StandardOutPath</key>\n");
    s.push_str(&format!(
        "    <string>{}</string>\n\n",
        xesc(&m.stdout_path)
    ));

    s.push_str("    <key>StandardErrorPath</key>\n");
    s.push_str(&format!(
        "    <string>{}</string>\n\n",
        xesc(&m.stderr_path)
    ));

    s.push_str("    <key>EnvironmentVariables</key>\n");
    s.push_str("    <dict>\n");
    // Fixed order matching the golden: PATH, VIGIL_STATE_DIR, VIGIL_LOG_DIR.
    for key in ["PATH", "VIGIL_STATE_DIR", "VIGIL_LOG_DIR"] {
        let val = m.env.get(key).map(String::as_str).unwrap_or("");
        s.push_str(&format!("        <key>{}</key>\n", xesc(key)));
        s.push_str(&format!("        <string>{}</string>\n", xesc(val)));
    }
    s.push_str("    </dict>\n");

    s.push_str("</dict>\n");
    s.push_str("</plist>\n");
    s
}

/// Render the system LaunchDaemon (helper) plist to its exact golden bytes
/// (`tests/golden/helper.plist`).
pub fn render_helper(cfg: &VigilConfig) -> Result<String, ServiceError> {
    let m = helper_model(cfg)?;
    let mut s = String::with_capacity(1024);
    s.push_str(PLIST_HEADER);

    s.push_str("    <key>Label</key>\n");
    s.push_str(&format!("    <string>{}</string>\n\n", xesc(&m.label)));

    s.push_str("    <key>ProgramArguments</key>\n");
    s.push_str("    <array>\n");
    for arg in &m.program_arguments {
        s.push_str(&format!("        <string>{}</string>\n", xesc(arg)));
    }
    s.push_str("    </array>\n\n");

    s.push_str("    <key>RunAtLoad</key>\n");
    s.push_str(&format!("    <{}/>\n\n", bool_tag(m.run_at_load)));

    s.push_str("    <key>KeepAlive</key>\n");
    s.push_str(&format!("    <{}/>\n\n", bool_tag(m.keep_alive)));

    s.push_str("    <key>ProcessType</key>\n");
    s.push_str(&format!(
        "    <string>{}</string>\n\n",
        xesc(&m.process_type)
    ));

    s.push_str("    <key>ExitTimeOut</key>\n");
    s.push_str(&format!("    <integer>{}</integer>\n\n", m.exit_timeout));

    s.push_str("    <key>ThrottleInterval</key>\n");
    s.push_str(&format!(
        "    <integer>{}</integer>\n\n",
        m.throttle_interval
    ));

    s.push_str("    <key>StandardOutPath</key>\n");
    s.push_str(&format!(
        "    <string>{}</string>\n\n",
        xesc(&m.stdout_path)
    ));

    s.push_str("    <key>StandardErrorPath</key>\n");
    s.push_str(&format!("    <string>{}</string>\n", xesc(&m.stderr_path)));

    s.push_str("</dict>\n");
    s.push_str("</plist>\n");
    Ok(s)
}

/// Render the `/etc/newsyslog.d/vigil.conf` rotation config to its exact golden
/// bytes (`tests/golden/vigil.newsyslog`). Rotation is owned by newsyslog (macOS
/// native), NEVER the tracing appender (§3.3). The owner group is the hardcoded
/// `staff` from the template; the user column is `id -un`.
pub fn render_newsyslog(cfg: &VigilConfig) -> Result<String, ServiceError> {
    let user = current_username()?;
    let mut s = String::with_capacity(1024);
    s.push_str("# /etc/newsyslog.d/vigil.conf — generated by `vigil setup`\n");
    s.push_str("#\n");
    s.push_str("# Rotates the bash daemon's primary log file (daemon.log). Does NOT touch\n");
    s.push_str("# daemon.out.log / daemon.err.log: those are held open by launchd's\n");
    s.push_str("# StandardOutPath / StandardErrorPath FDs, so rotation would leave launchd\n");
    s.push_str("# writing to the rotated file. The bash log() helper (lib/common.sh) reopens\n");
    s.push_str("# the path on every line, so rotation needs no SIGHUP handling.\n");
    s.push_str("#\n");
    s.push_str(
        "# Rotates at 1 MiB, keeps 5 generations, gzipped. `*` in the \"when\" column means\n",
    );
    s.push_str("# size-only, evaluated whenever com.apple.newsyslog fires (~hourly).\n");
    s.push_str("#\n");
    s.push_str(
        "# logfilename                  owner:group        mode  count  size  when  flags\n",
    );
    // The template substitutes @VIGIL_LOG_FILE@ and @VIGIL_USER@ verbatim (no XML
    // escaping — this is not XML). The fixed-width column layout is part of the
    // ABI and must match the template byte-for-byte.
    s.push_str(&format!(
        "{}               {}:staff 644   5      1024  *     GZ\n",
        cfg.log_file, user
    ));
    Ok(s)
}

fn bool_tag(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

// ── launchctl seam (testable without the real system) ─────────────────────────

/// The `launchctl` operations the installer needs, behind a trait so the
/// 50×100ms bootout poll is unit-testable without touching the real service
/// manager (spec §2.2.3, Q6). The real impl shells out; the test impl is a
/// scripted fake.
pub trait Launchctl {
    /// `launchctl print "{domain}/{label}"` — true iff it exits 0 (loaded).
    fn print_ok(&self, domain: &str, label: &str) -> bool;
    /// `launchctl bootout "{domain}/{label}"` — best-effort, errors ignored.
    fn bootout(&self, domain: &str, label: &str);
    /// `launchctl bootstrap "{domain}" "{plist}"`.
    fn bootstrap(&self, domain: &str, plist: &Path) -> Result<(), ServiceError>;
    /// `launchctl enable "{domain}/{label}"` — best-effort, errors ignored.
    fn enable(&self, domain: &str, label: &str);
    /// Sleep for the inter-poll interval (100ms in production; injectable so
    /// tests don't actually wait).
    fn sleep_poll(&self);
}

/// Production `launchctl` — shells out to the real binary; `sleep_poll` is a real
/// 100ms wall-clock sleep (correct in a command path, not the daemon loop).
pub struct RealLaunchctl;

impl Launchctl for RealLaunchctl {
    fn print_ok(&self, domain: &str, label: &str) -> bool {
        std::process::Command::new("launchctl")
            .arg("print")
            .arg(format!("{domain}/{label}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn bootout(&self, domain: &str, label: &str) {
        let _ = std::process::Command::new("launchctl")
            .arg("bootout")
            .arg(format!("{domain}/{label}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    fn bootstrap(&self, domain: &str, plist: &Path) -> Result<(), ServiceError> {
        std::process::Command::new("launchctl")
            .arg("bootstrap")
            .arg(domain)
            .arg(plist)
            .status()
            .map_err(|e| ServiceError::LaunchctlSpawn(e.to_string()))?;
        Ok(())
    }

    fn enable(&self, domain: &str, label: &str) {
        let _ = std::process::Command::new("launchctl")
            .arg("enable")
            .arg(format!("{domain}/{label}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    fn sleep_poll(&self) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// ── the portable installer seam ───────────────────────────────────────────────

/// Maximum bootout poll iterations (spec §2.2.3, Q6: EXACTLY 50). 50 × 100ms = 5s.
pub const BOOTOUT_POLL_MAX: usize = 50;

/// The portable service-management seam. Linux fills this in 5.8; only
/// [`MacosLaunchdInstaller`] exists in 5.7.
pub trait ServiceInstaller {
    /// Render the user agent plist to its canonical path and write it.
    fn install_user_agent(&self, cfg: &VigilConfig) -> Result<(), ServiceError>;
    /// Render the user agent plist to a String (for `setup --verbose`/`--dry-run`).
    fn render_user_agent(&self, cfg: &VigilConfig) -> Result<String, ServiceError>;
    /// Render the root LaunchDaemon (helper) plist to a String. Installation
    /// (sudo) is the caller's job.
    fn render_helper_daemon(&self, cfg: &VigilConfig) -> Result<String, ServiceError>;
    /// Render the newsyslog rotation config to a String.
    fn render_newsyslog(&self, cfg: &VigilConfig) -> Result<String, ServiceError>;
    /// Bootstrap (load) the user agent: `bootstrap` + best-effort `enable`.
    /// Idempotent: already-loaded → [`StartState::AlreadyLoaded`].
    fn start_user_agent(&self, cfg: &VigilConfig) -> Result<StartState, ServiceError>;
    /// Bootout the user agent with the 50×100ms poll. Idempotent.
    fn stop_user_agent(&self, cfg: &VigilConfig) -> Result<StopState, ServiceError>;
    /// True iff the agent is currently loaded (`launchctl print` succeeds).
    fn is_loaded(&self, cfg: &VigilConfig) -> bool;
}

/// macOS `launchd` implementation of [`ServiceInstaller`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct MacosLaunchdInstaller<L: Launchctl = RealLaunchctl> {
    launchctl: L,
}

impl Default for MacosLaunchdInstaller<RealLaunchctl> {
    fn default() -> Self {
        MacosLaunchdInstaller {
            launchctl: RealLaunchctl,
        }
    }
}

impl MacosLaunchdInstaller<RealLaunchctl> {
    /// Construct with the real, system-touching `launchctl`.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<L: Launchctl> MacosLaunchdInstaller<L> {
    /// Construct with a custom `launchctl` seam (tests inject a fake).
    pub fn with_launchctl(launchctl: L) -> Self {
        MacosLaunchdInstaller { launchctl }
    }

    /// `gui/{uid}` — the user agent's launchd domain.
    fn user_domain() -> String {
        format!("gui/{}", crate::config::get_uid())
    }

    /// `$HOME/Library/LaunchAgents/com.thangaram.vigil.plist`.
    fn user_plist_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{USER_AGENT_LABEL}.plist"))
    }
}

impl<L: Launchctl> ServiceInstaller for MacosLaunchdInstaller<L> {
    fn install_user_agent(&self, cfg: &VigilConfig) -> Result<(), ServiceError> {
        let rendered = render_user_agent(cfg);
        let path = Self::user_plist_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, rendered)?;
        Ok(())
    }

    fn render_user_agent(&self, cfg: &VigilConfig) -> Result<String, ServiceError> {
        Ok(render_user_agent(cfg))
    }

    fn render_helper_daemon(&self, cfg: &VigilConfig) -> Result<String, ServiceError> {
        render_helper(cfg)
    }

    fn render_newsyslog(&self, cfg: &VigilConfig) -> Result<String, ServiceError> {
        render_newsyslog(cfg)
    }

    fn start_user_agent(&self, _cfg: &VigilConfig) -> Result<StartState, ServiceError> {
        let plist = Self::user_plist_path();
        if !plist.is_file() {
            return Err(ServiceError::PlistMissing(plist.display().to_string()));
        }
        let domain = Self::user_domain();
        if self.launchctl.print_ok(&domain, USER_AGENT_LABEL) {
            return Ok(StartState::AlreadyLoaded);
        }
        self.launchctl.bootstrap(&domain, &plist)?;
        // `enable` is best-effort (bash ignores its failure).
        self.launchctl.enable(&domain, USER_AGENT_LABEL);
        Ok(StartState::Bootstrapped)
    }

    fn stop_user_agent(&self, cfg: &VigilConfig) -> Result<StopState, ServiceError> {
        let domain = Self::user_domain();
        if self.launchctl.print_ok(&domain, USER_AGENT_LABEL) {
            // bootout returns quickly but deregistration can outlast it; poll
            // until `print` fails, bounded at 50×100ms (spec §2.2.3, Q6).
            self.launchctl.bootout(&domain, USER_AGENT_LABEL);
            for _ in 0..BOOTOUT_POLL_MAX {
                if !self.launchctl.print_ok(&domain, USER_AGENT_LABEL) {
                    break;
                }
                self.launchctl.sleep_poll();
            }
            // best-effort tick-file removal (drop the stale snapshot).
            let _ = std::fs::remove_file(&cfg.daemon_tick_file);
            Ok(StopState::BootedOut)
        } else {
            let _ = std::fs::remove_file(&cfg.daemon_tick_file);
            Ok(StopState::NotLoaded)
        }
    }

    fn is_loaded(&self, _cfg: &VigilConfig) -> bool {
        let domain = Self::user_domain();
        self.launchctl.print_ok(&domain, USER_AGENT_LABEL)
    }
}

#[cfg(test)]
mod tests;
