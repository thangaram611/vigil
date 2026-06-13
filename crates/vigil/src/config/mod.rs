//! Config substrate for vigil — Phase 5.2.
//!
//! Loads `VigilConfig` via figment layers (lowest→highest priority):
//!   Serialized::defaults < Toml::file < Env::prefixed("VIGIL_").split("__") < CLI overrides
//!
//! After the merge three post-extraction passes run:
//!   1. `derive_provider_homes()` — exact bash cascade replica (see §2 of spec).
//!   2. `derive_paths()` — compute all derived path fields from resolved parents.
//!
//! SECURITY: vigil.conf is parsed as strict TOML (closes shell-injection-via-conf).
//! Security-path validation is in `validate_security_paths()` (called by admin
//! slices only, NOT by `config --show`).

use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

// ── Security constants (never overridable) ────────────────────────────────────

pub const VIGIL_ROOT: &str = "/Library/Application Support/vigil";
pub const NEWSYSLOG_FILE: &str = "/etc/newsyslog.d/vigil.conf";

// ── Config error ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ConfigError {
    ShellSyntax {
        path: String,
        hint: String,
    },
    #[allow(dead_code)]
    Parse(String),
    Figment(Box<figment::Error>),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::ShellSyntax { path, hint } => {
                write!(
                    f,
                    "vigil: {path}: not valid TOML — vigil.conf is now strict TOML, not \
                     shell. A line like 'export VIGIL_X=...' or '$HOME' is shell syntax. \
                     Convert to TOML: vigil_log_dir = \"/path\". {hint}"
                )
            }
            ConfigError::Parse(msg) => write!(f, "vigil: config parse error: {msg}"),
            ConfigError::Figment(e) => write!(f, "vigil: config error: {e}"),
        }
    }
}

impl From<figment::Error> for ConfigError {
    fn from(e: figment::Error) -> Self {
        ConfigError::Figment(Box::new(e))
    }
}

// ── Default helper functions ──────────────────────────────────────────────────

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

fn default_install_dir() -> String {
    format!("{}/Library/Application Support/vigil", home())
}

#[allow(dead_code)]
fn default_state_dir() -> String {
    format!("{}/state", default_install_dir())
}

fn default_log_dir() -> String {
    format!("{}/Library/Logs/vigil", home())
}

fn default_config_file() -> String {
    format!("{}/.config/vigil/vigil.conf", home())
}

fn default_lock_combo() -> String {
    "ctrl+alt+shift+cmd+l".to_string()
}

fn default_tick_secs() -> u32 {
    5
}
fn default_stale_age_secs() -> u32 {
    30
}
fn default_stale_cpu_pct() -> f64 {
    0.5
}
fn default_thermal_cooldown_secs() -> u32 {
    60
}
fn default_battery_floor_pct() -> u32 {
    20
}
fn default_start_wait_secs() -> u32 {
    6
}
fn default_lock_max_secs() -> u32 {
    28800
}
fn default_vscode_copilot_discover_secs() -> u32 {
    30
}
fn default_vscode_copilot_recent_mins() -> u32 {
    10
}
fn default_idle_after_sec() -> u32 {
    300
}
fn default_power_helper_timeout_secs() -> u32 {
    10
}

// ── Raw figment config (before post-processing) ───────────────────────────────
//
// Fields that are "cascade-derived" from a parent are `Option<String>` so we can
// detect when they were explicitly set vs. absent (= derive from parent).
// Fields that have simple scalar defaults are fully typed with #[serde(default)].

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConfig {
    // ---- user-controlled directories ----------------------------------------
    #[serde(default = "default_install_dir")]
    pub install_dir: String,

    // Option: derive from install_dir if None
    pub state_dir: Option<String>,

    #[serde(default = "default_log_dir")]
    pub log_dir: String,

    #[serde(default = "default_config_file")]
    pub config_file: String,

    // ---- root-tree paths (all Option: derive from root_dir if None) ----------
    pub root_dir: Option<String>,
    pub root_bin_dir: Option<String>,
    pub root_helper: Option<String>,
    pub power_helper_dir: Option<String>,
    pub power_request_base: Option<String>,
    pub power_response_base: Option<String>,
    pub power_request_dir: Option<String>,
    pub power_response_dir: Option<String>,
    pub power_state_dir: Option<String>,
    pub power_log_dir: Option<String>,
    pub power_log_file: Option<String>,

    #[serde(default = "default_power_helper_timeout_secs")]
    pub power_helper_timeout_secs: u32,

    // ---- tunables -----------------------------------------------------------
    #[serde(default = "default_tick_secs")]
    pub tick_secs: u32,

    #[serde(default = "default_stale_age_secs")]
    pub stale_age_secs: u32,

    #[serde(default = "default_stale_cpu_pct")]
    pub stale_cpu_pct: f64,

    #[serde(default = "default_thermal_cooldown_secs")]
    pub thermal_cooldown_secs: u32,

    #[serde(default = "default_battery_floor_pct")]
    pub battery_floor_pct: u32,

    #[serde(default = "default_start_wait_secs")]
    pub start_wait_secs: u32,

    #[serde(default = "default_lock_combo")]
    pub lock_combo: String,

    #[serde(default = "default_lock_max_secs")]
    pub lock_max_secs: u32,

    // Option: derive from install_dir if None
    pub lock_helper: Option<String>,

    // ---- provider homes (Option = unset → cascade) --------------------------
    pub claude_home: Option<String>,
    pub codex_home: Option<String>,
    pub copilot_home: Option<String>,

    // ---- provider-env passthroughs from TOML --------------------------------
    // Allows vigil.conf TOML to set CLAUDE_CONFIG_DIR / CODEX_HOME / COPILOT_HOME
    // (bash conf could set these as shell vars; TOML can't set process env, so
    // these passthrough keys replicate that behavior).
    pub claude_config_dir: Option<String>,
    pub codex_home_env: Option<String>,
    pub copilot_home_env: Option<String>,

    // ---- vscode copilot discovery -------------------------------------------
    #[serde(default = "default_vscode_copilot_discover_secs")]
    pub vscode_copilot_discover_secs: u32,

    #[serde(default = "default_vscode_copilot_recent_mins")]
    pub vscode_copilot_recent_mins: u32,

    // ---- idle/force ---------------------------------------------------------
    #[serde(default = "default_idle_after_sec")]
    pub idle_after_sec: u32,

    #[serde(default)]
    pub force: u8,
}

impl Default for RawConfig {
    fn default() -> Self {
        RawConfig {
            install_dir: default_install_dir(),
            state_dir: None,
            log_dir: default_log_dir(),
            config_file: default_config_file(),
            root_dir: None,
            root_bin_dir: None,
            root_helper: None,
            power_helper_dir: None,
            power_request_base: None,
            power_response_base: None,
            power_request_dir: None,
            power_response_dir: None,
            power_state_dir: None,
            power_log_dir: None,
            power_log_file: None,
            power_helper_timeout_secs: default_power_helper_timeout_secs(),
            tick_secs: default_tick_secs(),
            stale_age_secs: default_stale_age_secs(),
            stale_cpu_pct: default_stale_cpu_pct(),
            thermal_cooldown_secs: default_thermal_cooldown_secs(),
            battery_floor_pct: default_battery_floor_pct(),
            start_wait_secs: default_start_wait_secs(),
            lock_combo: default_lock_combo(),
            lock_max_secs: default_lock_max_secs(),
            lock_helper: None,
            claude_home: None,
            codex_home: None,
            copilot_home: None,
            claude_config_dir: None,
            codex_home_env: None,
            copilot_home_env: None,
            vscode_copilot_discover_secs: default_vscode_copilot_discover_secs(),
            vscode_copilot_recent_mins: default_vscode_copilot_recent_mins(),
            idle_after_sec: default_idle_after_sec(),
            force: 0,
        }
    }
}

// ── Resolved config (post-processing complete) ────────────────────────────────

#[derive(Debug, Clone)]
pub struct VigilConfig {
    // ---- user-controlled directories ----------------------------------------
    pub install_dir: String,
    pub state_dir: String,
    pub log_dir: String,
    pub config_file: String,

    // ---- state-dir derived paths (unconditionally re-derived from state_dir) -
    pub active_dir: String,
    pub baseline_file: String,
    pub caffeinate_pidfile: String,
    pub daemon_pidfile: String,
    pub daemon_tick_file: String,
    pub lock_file: String,
    pub vscode_copilot_state_file: String,

    // ---- log-dir derived path -----------------------------------------------
    pub log_file: String,

    // ---- root-tree paths ----------------------------------------------------
    pub root_dir: String,
    pub root_bin_dir: String,
    pub root_helper: String,
    pub power_helper_dir: String,
    pub power_request_base: String,
    pub power_response_base: String,
    pub power_request_dir: String,
    pub power_response_dir: String,
    pub power_state_dir: String,
    pub power_log_dir: String,
    pub power_log_file: String,
    pub power_helper_timeout_secs: u32,

    // ---- newsyslog (hardcoded constant — NOT overridable) -------------------
    /// Always `/etc/newsyslog.d/vigil.conf`. Hardcoded, never read from env/conf.
    pub newsyslog_file: String,

    // ---- tunables -----------------------------------------------------------
    pub tick_secs: u32,
    pub stale_age_secs: u32,
    pub stale_cpu_pct: f64,
    pub thermal_cooldown_secs: u32,
    pub battery_floor_pct: u32,
    pub start_wait_secs: u32,
    pub lock_combo: String,
    pub lock_max_secs: u32,
    pub lock_helper: String,

    // ---- provider homes (resolved; auto flags) ------------------------------
    pub claude_home: String,
    pub claude_home_auto: bool,
    pub codex_home: String,
    pub codex_home_auto: bool,
    pub copilot_home: String,
    pub copilot_home_auto: bool,

    // ---- vscode copilot discovery -------------------------------------------
    pub vscode_copilot_discover_secs: u32,
    pub vscode_copilot_recent_mins: u32,

    // ---- idle/force ---------------------------------------------------------
    pub idle_after_sec: u32,
    pub force: u8,
}

// ── CLI overrides (passed after Env layer) ────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CliOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_after_sec: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<u8>,
}

// ── get_uid ───────────────────────────────────────────────────────────────────

pub fn get_uid() -> u32 {
    // SAFETY: geteuid() is always safe; no invariants to uphold.
    unsafe { libc::geteuid() }
}

// ── Provider-home cascade — exact bash algorithm ──────────────────────────────
//
// Reproduces bash lines 72–86 (source-time) + 230–249 (vigil_load_config).
// See §2 of spec for the full decision table (Cases A–H).

/// Derive one provider's home directory and auto flag.
///
/// - `vigil_home_field`: merged `VIGIL_*_HOME` value (from toml or env via figment).
/// - `vigil_home_from_process_env`: whether the process env had `VIGIL_*_HOME` set.
/// - `provider_env_effective`: conf passthrough if Some, else live process env var.
/// - `default`: the auto-default home (e.g. `$HOME/.claude`).
///
/// Returns `(resolved_home, auto_flag)`.
fn derive_one_home(
    vigil_home_field: Option<String>,
    vigil_home_from_process_env: Option<String>,
    provider_env_effective: Option<String>,
    default: String,
) -> (String, bool) {
    match vigil_home_field {
        Some(explicit) => {
            if vigil_home_from_process_env.is_some() {
                // Cases C, E: env-explicit. AUTO=0, NEVER clobbered.
                (explicit, false)
            } else {
                // toml-only-explicit (Cases F, G, H).
                if explicit == default {
                    // Case H pathological: value == auto-default → provider-env wins.
                    (provider_env_effective.unwrap_or(default), true)
                } else {
                    // Case F/G: toml VIGIL_*_HOME wins over conf provider-env.
                    (explicit, true)
                }
            }
        }
        None => {
            // Cases A, B, D: unset everywhere → auto cascade.
            (provider_env_effective.unwrap_or(default), true)
        }
    }
}

fn derive_provider_homes(raw: &RawConfig) -> (String, bool, String, bool, String, bool) {
    let h = home();
    let uid_str = get_uid().to_string();
    let _ = uid_str; // not used here; suppress warning

    // Claude
    let claude_default = format!("{}/.claude", h);
    let claude_vigil_env = std::env::var("VIGIL_CLAUDE_HOME").ok();
    let claude_provider_effective = raw
        .claude_config_dir
        .clone()
        .or_else(|| std::env::var("CLAUDE_CONFIG_DIR").ok());
    let (claude_home, claude_auto) = derive_one_home(
        raw.claude_home.clone(),
        claude_vigil_env,
        claude_provider_effective,
        claude_default,
    );

    // Codex
    let codex_default = format!("{}/.codex", h);
    let codex_vigil_env = std::env::var("VIGIL_CODEX_HOME").ok();
    let codex_provider_effective = raw
        .codex_home_env
        .clone()
        .or_else(|| std::env::var("CODEX_HOME").ok());
    let (codex_home, codex_auto) = derive_one_home(
        raw.codex_home.clone(),
        codex_vigil_env,
        codex_provider_effective,
        codex_default,
    );

    // Copilot
    let copilot_default = format!("{}/.copilot", h);
    let copilot_vigil_env = std::env::var("VIGIL_COPILOT_HOME").ok();
    let copilot_provider_effective = raw
        .copilot_home_env
        .clone()
        .or_else(|| std::env::var("COPILOT_HOME").ok());
    let (copilot_home, copilot_auto) = derive_one_home(
        raw.copilot_home.clone(),
        copilot_vigil_env,
        copilot_provider_effective,
        copilot_default,
    );

    (
        claude_home,
        claude_auto,
        codex_home,
        codex_auto,
        copilot_home,
        copilot_auto,
    )
}

// ── Path derivation ───────────────────────────────────────────────────────────

fn derive_paths(raw: RawConfig) -> VigilConfig {
    let uid = get_uid().to_string();
    let (claude_home, claude_auto, codex_home, codex_auto, copilot_home, copilot_auto) =
        derive_provider_homes(&raw);

    // --- cascade-derived optional fields (Option = derive from parent) -------
    let state_dir = raw
        .state_dir
        .unwrap_or_else(|| format!("{}/state", raw.install_dir));

    let root_dir = raw.root_dir.unwrap_or_else(|| VIGIL_ROOT.to_string());
    let root_bin_dir = raw
        .root_bin_dir
        .unwrap_or_else(|| format!("{}/bin", root_dir));
    let root_helper = raw
        .root_helper
        .unwrap_or_else(|| format!("{}/vigil-root-helper", root_bin_dir));
    let power_helper_dir = raw
        .power_helper_dir
        .unwrap_or_else(|| format!("{}/helper", root_dir));
    let power_request_base = raw
        .power_request_base
        .unwrap_or_else(|| format!("{}/requests", power_helper_dir));
    let power_response_base = raw
        .power_response_base
        .unwrap_or_else(|| format!("{}/responses", power_helper_dir));
    let power_request_dir = raw
        .power_request_dir
        .unwrap_or_else(|| format!("{}/{}", power_request_base, uid));
    let power_response_dir = raw
        .power_response_dir
        .unwrap_or_else(|| format!("{}/{}", power_response_base, uid));
    let power_state_dir = raw
        .power_state_dir
        .unwrap_or_else(|| format!("{}/state", power_helper_dir));
    let power_log_dir = raw
        .power_log_dir
        .unwrap_or_else(|| format!("{}/logs", power_helper_dir));
    let power_log_file = raw
        .power_log_file
        .unwrap_or_else(|| format!("{}/helper.log", power_log_dir));

    let lock_helper = raw
        .lock_helper
        .unwrap_or_else(|| format!("{}/bin/vigil-lock-helper", raw.install_dir));

    // --- unconditional re-derives from resolved parents ----------------------
    // log_file is ALWAYS re-derived from log_dir (bash line 256, unconditional)
    let log_file = format!("{}/daemon.log", raw.log_dir);

    // state-subpaths are pure functions of the resolved state_dir
    let active_dir = format!("{}/active", state_dir);
    let baseline_file = format!("{}/baseline.json", state_dir);
    let caffeinate_pidfile = format!("{}/caffeinate.pid", state_dir);
    let daemon_pidfile = format!("{}/daemon.pid", state_dir);
    let daemon_tick_file = format!("{}/daemon.tick", state_dir);
    let lock_file_path = format!("{}/state.lock", state_dir);
    let vscode_copilot_state_file = format!("{}/vscode-copilot-chat.state", state_dir);

    VigilConfig {
        install_dir: raw.install_dir,
        state_dir,
        log_dir: raw.log_dir,
        config_file: raw.config_file,
        active_dir,
        baseline_file,
        caffeinate_pidfile,
        daemon_pidfile,
        daemon_tick_file,
        lock_file: lock_file_path,
        vscode_copilot_state_file,
        log_file,
        root_dir,
        root_bin_dir,
        root_helper,
        power_helper_dir,
        power_request_base,
        power_response_base,
        power_request_dir,
        power_response_dir,
        power_state_dir,
        power_log_dir,
        power_log_file,
        power_helper_timeout_secs: raw.power_helper_timeout_secs,
        newsyslog_file: NEWSYSLOG_FILE.to_string(),
        tick_secs: raw.tick_secs,
        stale_age_secs: raw.stale_age_secs,
        stale_cpu_pct: raw.stale_cpu_pct,
        thermal_cooldown_secs: raw.thermal_cooldown_secs,
        battery_floor_pct: raw.battery_floor_pct,
        start_wait_secs: raw.start_wait_secs,
        lock_combo: raw.lock_combo,
        lock_max_secs: raw.lock_max_secs,
        lock_helper,
        claude_home,
        claude_home_auto: claude_auto,
        codex_home,
        codex_home_auto: codex_auto,
        copilot_home,
        copilot_home_auto: copilot_auto,
        vscode_copilot_discover_secs: raw.vscode_copilot_discover_secs,
        vscode_copilot_recent_mins: raw.vscode_copilot_recent_mins,
        idle_after_sec: raw.idle_after_sec,
        force: raw.force,
    }
}

// ── Shell-syntax detection ────────────────────────────────────────────────────

fn looks_like_shell_conf(path: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        // `#!/...` — shebang, unmistakably shell. Must be checked BEFORE the
        // comment-skip below: a shebang starts with `#` and a TOML parser treats
        // it as a comment, so a conf that begins with `#!/usr/bin/env bash` and
        // has no other shell indicators would otherwise slip through silently.
        if trimmed.starts_with("#!") {
            return Some(format!("Detected shell syntax near: {trimmed:?}"));
        }
        // Skip comments and blank lines.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // `export VIGIL_X=...` — unambiguous shell syntax.
        if trimmed.starts_with("export ") {
            return Some(format!("Detected shell syntax near: {trimmed:?}"));
        }
        // `$VAR` anywhere — shell variable expansion, invalid in TOML values.
        if trimmed.contains('$') {
            return Some(format!("Detected shell syntax near: {trimmed:?}"));
        }
        // `VIGIL_SOMETHING="..."` — bare env-var assignment without quotes around
        // the key. In TOML, keys are bare words (letters, digits, `-`, `_`) but
        // a key like `VIGIL_LOG_DIR` is valid TOML too. However the bash conf
        // style uses ALL_CAPS_WITH_UNDERSCORES="..." which differs from TOML
        // style (lowercase_snake_case = "..."). Detect: key is all-uppercase +
        // underscores with no dots/dashes, immediately followed by `=`.
        if trimmed.contains('=') {
            let key = trimmed.split('=').next().unwrap_or("").trim();
            // key is all [A-Z_] and starts with A-Z → shell var name style
            if !key.is_empty()
                && key.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                && key.chars().next().unwrap().is_ascii_uppercase()
            {
                return Some(format!("Detected shell syntax near: {trimmed:?}"));
            }
        }
    }
    None
}

// ── Main load function ────────────────────────────────────────────────────────

/// Load and fully resolve `VigilConfig`.
///
/// Layering (lowest→highest): defaults < Toml(conf_path) < Env(VIGIL_) < CLI.
/// Then: derive_provider_homes() → derive_paths().
///
/// `conf_path`: path to the TOML config file. If the file does not exist, that
/// layer is silently skipped (figment behavior). If the file exists but fails
/// to parse as TOML (or contains shell syntax), returns a clear error.
pub fn load(conf_path: &str, cli: Option<CliOverrides>) -> Result<VigilConfig, ConfigError> {
    // Proactively check for shell-syntax conf: bash conf can contain `export`,
    // `$VAR`, `#!/...` etc. which are NOT valid TOML semantics even if some
    // constructs accidentally parse as TOML. We check before figment so we can
    // give a targeted error message.
    if Path::new(conf_path).exists() {
        if let Some(hint) = looks_like_shell_conf(conf_path) {
            return Err(ConfigError::ShellSyntax {
                path: conf_path.to_string(),
                hint,
            });
        }
        // Also attempt a TOML parse to catch syntax errors not covered by the
        // shell-pattern heuristic above.
        let test_result: Result<RawConfig, _> =
            Figment::from(Serialized::defaults(RawConfig::default()))
                .merge(Toml::file(conf_path))
                .extract();
        if test_result.is_err() {
            return Err(ConfigError::ShellSyntax {
                path: conf_path.to_string(),
                hint: "Check the file for shell-specific syntax.".to_string(),
            });
        }
    }

    let mut fig = Figment::from(Serialized::defaults(RawConfig::default()))
        .merge(Toml::file(conf_path))
        // CRITICAL: split("__") NOT split("_"). Using the default "_" splitter
        // would turn VIGIL_IDLE_AFTER_SEC into nested idle.after.sec and the
        // flat field would never bind.
        .merge(Env::prefixed("VIGIL_").split("__"));

    if let Some(cli_opts) = cli {
        fig = fig.merge(Serialized::defaults(cli_opts));
    }

    let raw: RawConfig = fig
        .extract()
        .map_err(|e| ConfigError::Figment(Box::new(e)))?;
    Ok(derive_paths(raw))
}

/// Convenience: load using the default config file path (from env or default).
#[allow(dead_code)]
pub fn load_default() -> Result<VigilConfig, ConfigError> {
    let conf_path = std::env::var("VIGIL_CONFIG_FILE").unwrap_or_else(|_| default_config_file());
    load(&conf_path, None)
}

// ── Security-path allowlist ───────────────────────────────────────────────────

impl VigilConfig {
    /// Validate all security-critical paths against their canonical hardcoded
    /// values using EXACT string equality (NOT prefix). Call from admin paths
    /// only; NEVER from `config --show`.
    #[allow(dead_code)]
    pub fn validate_security_paths(&self) -> Result<(), String> {
        let uid = get_uid().to_string();
        let root = VIGIL_ROOT;

        macro_rules! check {
            ($label:expr, $actual:expr, $expected:expr) => {{
                let expected: String = $expected;
                if $actual != expected {
                    return Err(format!(
                        "refusing non-standard {} for privileged operation: {}",
                        $label, $actual
                    ));
                }
            }};
        }

        check!("VIGIL_ROOT_DIR", self.root_dir, root.to_string());
        check!(
            "VIGIL_ROOT_BIN_DIR",
            self.root_bin_dir,
            format!("{root}/bin")
        );
        check!(
            "VIGIL_ROOT_HELPER",
            self.root_helper,
            format!("{root}/bin/vigil-root-helper")
        );
        check!(
            "VIGIL_POWER_HELPER_DIR",
            self.power_helper_dir,
            format!("{root}/helper")
        );
        check!(
            "VIGIL_POWER_REQUEST_BASE",
            self.power_request_base,
            format!("{root}/helper/requests")
        );
        check!(
            "VIGIL_POWER_RESPONSE_BASE",
            self.power_response_base,
            format!("{root}/helper/responses")
        );
        check!(
            "VIGIL_POWER_REQUEST_DIR",
            self.power_request_dir,
            format!("{root}/helper/requests/{uid}")
        );
        check!(
            "VIGIL_POWER_RESPONSE_DIR",
            self.power_response_dir,
            format!("{root}/helper/responses/{uid}")
        );
        check!(
            "VIGIL_POWER_STATE_DIR",
            self.power_state_dir,
            format!("{root}/helper/state")
        );
        check!(
            "VIGIL_POWER_LOG_DIR",
            self.power_log_dir,
            format!("{root}/helper/logs")
        );
        check!(
            "VIGIL_POWER_LOG_FILE",
            self.power_log_file,
            format!("{root}/helper/logs/helper.log")
        );
        check!(
            "VIGIL_NEWSYSLOG_FILE",
            self.newsyslog_file,
            NEWSYSLOG_FILE.to_string()
        );

        // The helper plist path is a bin/vigil constant, not part of VigilConfig.
        // Plist checks live in the admin slices (5.5/5.7).
        Ok(())
    }

    /// Create the state directory (and active subdir + log dir) with mode 0700
    /// on the state dir. Only call from daemon/setup paths; NEVER from
    /// `config --show`.
    #[allow(dead_code)]
    pub fn ensure_state_dir(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::create_dir_all(&self.active_dir)?;
        std::fs::create_dir_all(&self.log_dir)?;
        // chmod 0700 state_dir only (bash vigil_ensure_dirs line 263)
        let meta = std::fs::metadata(&self.state_dir)?;
        let mut perms = meta.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&self.state_dir, perms)?;
        Ok(())
    }

    /// Convert the fully-resolved config to a stable sorted BTreeMap<String, String>
    /// using VIGIL_* env-var names as keys. Used by `config --show`/`--json`/`--kv`.
    pub fn to_kv_map(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("VIGIL_ACTIVE_DIR".into(), self.active_dir.clone());
        m.insert("VIGIL_BASELINE_FILE".into(), self.baseline_file.clone());
        m.insert(
            "VIGIL_BATTERY_FLOOR_PCT".into(),
            self.battery_floor_pct.to_string(),
        );
        m.insert(
            "VIGIL_CAFFEINATE_PIDFILE".into(),
            self.caffeinate_pidfile.clone(),
        );
        m.insert("VIGIL_CLAUDE_HOME".into(), self.claude_home.clone());
        m.insert(
            "VIGIL_CLAUDE_HOME_AUTO".into(),
            if self.claude_home_auto { "1" } else { "0" }.to_string(),
        );
        m.insert("VIGIL_CODEX_HOME".into(), self.codex_home.clone());
        m.insert(
            "VIGIL_CODEX_HOME_AUTO".into(),
            if self.codex_home_auto { "1" } else { "0" }.to_string(),
        );
        m.insert("VIGIL_CONFIG_FILE".into(), self.config_file.clone());
        m.insert("VIGIL_COPILOT_HOME".into(), self.copilot_home.clone());
        m.insert(
            "VIGIL_COPILOT_HOME_AUTO".into(),
            if self.copilot_home_auto { "1" } else { "0" }.to_string(),
        );
        m.insert("VIGIL_DAEMON_PIDFILE".into(), self.daemon_pidfile.clone());
        m.insert(
            "VIGIL_DAEMON_TICK_FILE".into(),
            self.daemon_tick_file.clone(),
        );
        m.insert("VIGIL_FORCE".into(), self.force.to_string());
        m.insert(
            "VIGIL_IDLE_AFTER_SEC".into(),
            self.idle_after_sec.to_string(),
        );
        m.insert("VIGIL_INSTALL_DIR".into(), self.install_dir.clone());
        m.insert("VIGIL_LOCK_COMBO".into(), self.lock_combo.clone());
        m.insert("VIGIL_LOCK_FILE".into(), self.lock_file.clone());
        m.insert("VIGIL_LOCK_HELPER".into(), self.lock_helper.clone());
        m.insert("VIGIL_LOCK_MAX_SECS".into(), self.lock_max_secs.to_string());
        m.insert("VIGIL_LOG_DIR".into(), self.log_dir.clone());
        m.insert("VIGIL_LOG_FILE".into(), self.log_file.clone());
        m.insert("VIGIL_NEWSYSLOG_FILE".into(), self.newsyslog_file.clone());
        m.insert(
            "VIGIL_POWER_HELPER_DIR".into(),
            self.power_helper_dir.clone(),
        );
        m.insert(
            "VIGIL_POWER_HELPER_TIMEOUT_SECS".into(),
            self.power_helper_timeout_secs.to_string(),
        );
        m.insert("VIGIL_POWER_LOG_DIR".into(), self.power_log_dir.clone());
        m.insert("VIGIL_POWER_LOG_FILE".into(), self.power_log_file.clone());
        m.insert(
            "VIGIL_POWER_REQUEST_BASE".into(),
            self.power_request_base.clone(),
        );
        m.insert(
            "VIGIL_POWER_REQUEST_DIR".into(),
            self.power_request_dir.clone(),
        );
        m.insert(
            "VIGIL_POWER_RESPONSE_BASE".into(),
            self.power_response_base.clone(),
        );
        m.insert(
            "VIGIL_POWER_RESPONSE_DIR".into(),
            self.power_response_dir.clone(),
        );
        m.insert("VIGIL_POWER_STATE_DIR".into(), self.power_state_dir.clone());
        m.insert("VIGIL_ROOT_BIN_DIR".into(), self.root_bin_dir.clone());
        m.insert("VIGIL_ROOT_DIR".into(), self.root_dir.clone());
        m.insert("VIGIL_ROOT_HELPER".into(), self.root_helper.clone());
        m.insert(
            "VIGIL_STALE_AGE_SECS".into(),
            self.stale_age_secs.to_string(),
        );
        m.insert(
            "VIGIL_STALE_CPU_PCT".into(),
            format!("{}", self.stale_cpu_pct),
        );
        m.insert(
            "VIGIL_START_WAIT_SECS".into(),
            self.start_wait_secs.to_string(),
        );
        m.insert("VIGIL_STATE_DIR".into(), self.state_dir.clone());
        m.insert(
            "VIGIL_THERMAL_COOLDOWN_SECS".into(),
            self.thermal_cooldown_secs.to_string(),
        );
        m.insert("VIGIL_TICK_SECS".into(), self.tick_secs.to_string());
        m.insert(
            "VIGIL_VSCODE_COPILOT_DISCOVER_SECS".into(),
            self.vscode_copilot_discover_secs.to_string(),
        );
        m.insert(
            "VIGIL_VSCODE_COPILOT_RECENT_MINS".into(),
            self.vscode_copilot_recent_mins.to_string(),
        );
        m.insert(
            "VIGIL_VSCODE_COPILOT_STATE_FILE".into(),
            self.vscode_copilot_state_file.clone(),
        );
        m
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    // Serialize env-mutating tests: parallel test threads share process env.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Acquire the env lock, recovering from a poisoned mutex (caused by a
    /// previous test panic while holding the lock). This avoids cascading
    /// failures across serialized env-mutation tests.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn make_tmp_home() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // ── Env-split footgun guard ───────────────────────────────────────────────

    #[test]
    fn env_split_idle_after_sec() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        let conf = tmp.path().join("vigil.conf");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("VIGIL_IDLE_AFTER_SEC", "999");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf.to_str().unwrap(), None).expect("load");
        unsafe {
            std::env::remove_var("VIGIL_IDLE_AFTER_SEC");
        }
        assert_eq!(
            cfg.idle_after_sec, 999,
            "VIGIL_IDLE_AFTER_SEC must bind via __ split"
        );
    }

    #[test]
    fn env_split_power_helper_timeout() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        let conf = tmp.path().join("vigil.conf");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("VIGIL_POWER_HELPER_TIMEOUT_SECS", "42");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf.to_str().unwrap(), None).expect("load");
        unsafe {
            std::env::remove_var("VIGIL_POWER_HELPER_TIMEOUT_SECS");
        }
        assert_eq!(cfg.power_helper_timeout_secs, 42);
    }

    // ── Provider-home cascade cases ───────────────────────────────────────────

    #[test]
    fn case_c_env_explicit_vigil_claude_home_not_clobbered() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        let conf = tmp.path().join("vigil.conf");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("VIGIL_CLAUDE_HOME", "/explicit/claude");
            std::env::set_var("CLAUDE_CONFIG_DIR", "/provider/claude");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf.to_str().unwrap(), None).expect("load");
        unsafe {
            std::env::remove_var("VIGIL_CLAUDE_HOME");
            std::env::remove_var("CLAUDE_CONFIG_DIR");
        }
        assert_eq!(
            cfg.claude_home, "/explicit/claude",
            "Case C: env-explicit must win"
        );
        assert!(
            !cfg.claude_home_auto,
            "Case C: AUTO must be 0 when env-explicit"
        );
    }

    #[test]
    fn case_e_env_explicit_survives_conf_provider_env() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        // Write toml conf with claude_config_dir passthrough
        let conf_path = tmp.path().join("vigil.conf");
        {
            let mut f = std::fs::File::create(&conf_path).unwrap();
            writeln!(f, r#"claude_config_dir = "/from/conf/provider""#).unwrap();
        }
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("VIGIL_CLAUDE_HOME", "/explicit/env");
            std::env::remove_var("CLAUDE_CONFIG_DIR");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf_path.to_str().unwrap(), None).expect("load");
        unsafe {
            std::env::remove_var("VIGIL_CLAUDE_HOME");
        }
        assert_eq!(
            cfg.claude_home, "/explicit/env",
            "Case E: env-explicit survives conf-set provider-env"
        );
        assert!(!cfg.claude_home_auto, "Case E: AUTO must be 0");
    }

    #[test]
    fn case_g_toml_vigil_claude_home_beats_toml_provider_env() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        let conf_path = tmp.path().join("vigil.conf");
        {
            let mut f = std::fs::File::create(&conf_path).unwrap();
            writeln!(f, r#"claude_home = "/toml/explicit""#).unwrap();
            writeln!(f, r#"claude_config_dir = "/toml/provider""#).unwrap();
        }
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("VIGIL_CLAUDE_HOME");
            std::env::remove_var("CLAUDE_CONFIG_DIR");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf_path.to_str().unwrap(), None).expect("load");
        assert_eq!(
            cfg.claude_home, "/toml/explicit",
            "Case G: toml VIGIL_CLAUDE_HOME beats toml claude_config_dir"
        );
        assert!(
            cfg.claude_home_auto,
            "Case G: AUTO stays 1 for toml-explicit"
        );
    }

    // ── log-file unconditional re-derive (M1 must-pass) ───────────────────────

    #[test]
    fn log_file_rederived_from_log_dir_in_conf() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        let custom_log = tmp.path().join("customlogs");
        let conf_path = tmp.path().join("vigil.conf");
        {
            let mut f = std::fs::File::create(&conf_path).unwrap();
            // TOML key is the serde field name (lowercase snake_case after VIGIL_ strip)
            writeln!(f, r#"log_dir = "{}""#, custom_log.display()).unwrap();
        }
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("VIGIL_LOG_DIR");
            std::env::remove_var("VIGIL_LOG_FILE");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf_path.to_str().unwrap(), None).expect("load");
        let expected = format!("{}/daemon.log", custom_log.display());
        assert_eq!(
            cfg.log_file, expected,
            "M1: VIGIL_LOG_FILE must re-derive from conf-set log_dir"
        );
    }

    // ── Security allowlist ────────────────────────────────────────────────────

    #[test]
    fn security_default_passes() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        let conf = tmp.path().join("vigil.conf");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("VIGIL_ROOT_DIR");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf.to_str().unwrap(), None).expect("load");
        assert!(
            cfg.validate_security_paths().is_ok(),
            "default config must pass security validation"
        );
    }

    #[test]
    fn security_evil_root_dir_rejected() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        let conf = tmp.path().join("vigil.conf");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("VIGIL_ROOT_DIR", "/tmp/evil");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf.to_str().unwrap(), None).expect("load");
        unsafe {
            std::env::remove_var("VIGIL_ROOT_DIR");
        }
        let result = cfg.validate_security_paths();
        assert!(result.is_err(), "non-standard root_dir must be rejected");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("refusing non-standard VIGIL_ROOT_DIR"),
            "error message must identify the field: {msg}"
        );
    }

    #[test]
    fn security_prefix_root_dir_rejected() {
        let _g = lock_env();
        let tmp = make_tmp_home();
        let home = tmp.path().to_str().unwrap();
        let conf = tmp.path().join("vigil.conf");
        unsafe {
            std::env::set_var("HOME", home);
            // A prefix of the canonical path — must fail exact-equality check.
            std::env::set_var("VIGIL_ROOT_DIR", "/Library/Application Support/vigil-evil");
            std::env::remove_var("VIGIL_CONFIG_FILE");
        }
        let cfg = load(conf.to_str().unwrap(), None).expect("load");
        unsafe {
            std::env::remove_var("VIGIL_ROOT_DIR");
        }
        let result = cfg.validate_security_paths();
        assert!(
            result.is_err(),
            "prefix-of-canonical root_dir must be rejected (exact-equality)"
        );
    }

    // ── Shell-syntax conf detection ───────────────────────────────────────────

    #[test]
    fn shell_syntax_conf_gives_clear_error() {
        let tmp = make_tmp_home();
        let conf_path = tmp.path().join("vigil.conf");
        {
            let mut f = std::fs::File::create(&conf_path).unwrap();
            // Shell-style: ALL_CAPS key + `=` + `$HOME` expansion — should be rejected.
            writeln!(f, r#"VIGIL_LOG_DIR="$HOME/logs""#).unwrap();
        }
        let result = load(conf_path.to_str().unwrap(), None);
        assert!(result.is_err(), "shell-syntax conf must error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not valid TOML"),
            "error must mention TOML: {msg}"
        );
    }

    #[test]
    fn shebang_only_conf_gives_clear_error() {
        // A conf that begins with a shebang and has NO other shell indicators.
        // The shebang is a valid TOML comment, so without the shebang branch
        // running BEFORE the generic `#` comment-skip this would parse as empty
        // TOML and silently succeed. This guards that the shebang check is live.
        let tmp = make_tmp_home();
        let conf_path = tmp.path().join("vigil.conf");
        {
            let mut f = std::fs::File::create(&conf_path).unwrap();
            writeln!(f, "#!/usr/bin/env bash").unwrap();
            // Otherwise-valid TOML below — only the shebang flags this as shell.
            writeln!(f, "idle_after_sec = 42").unwrap();
        }
        let result = load(conf_path.to_str().unwrap(), None);
        assert!(
            result.is_err(),
            "shebang-prefixed conf must be detected as shell syntax"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not valid TOML"),
            "error must mention TOML: {msg}"
        );
    }
}
