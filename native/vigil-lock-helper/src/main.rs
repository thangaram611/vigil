use std::env;

mod combo;
#[cfg(target_os = "macos")]
mod macos;

const EXIT_OK: i32 = 0;
#[cfg(not(target_os = "macos"))]
const EXIT_UNSUPPORTED: i32 = 10;
const EXIT_PERMISSION_FAIL: i32 = 20;
const EXIT_INVALID_ARGS: i32 = 30;
const EXIT_TAP_FAIL: i32 = 40;
const EXIT_WATCHDOG_FAIL: i32 = 50;

#[derive(Debug, PartialEq, Eq)]
enum Command {
    CheckPermissions {
        json: bool,
        prompt: bool,
    },
    Freeze {
        combo: String,
        max_secs: u64,
        debug_sleep_ms: Option<u64>,
    },
}

fn print_usage() {
    eprintln!("vigil-lock-helper --check-permissions --json [--prompt]");
    eprintln!("vigil-lock-helper --freeze --combo <combo> --max-secs <seconds> [--debug-sleep-in-callback-ms <ms>]");
}

fn parse_u64(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid integer value: {value}"))
}

fn parse_args(args: Vec<String>) -> Result<Command, String> {
    let mut i = 0;
    let mut mode: Option<Command> = None;
    let mut json = false;
    let mut prompt = false;
    let mut combo = None;
    let mut max_secs = None;
    let mut debug_sleep_ms = None;

    while i < args.len() {
        match args[i].as_str() {
            "--check-permissions" => {
                mode = Some(Command::CheckPermissions {
                    json: true,
                    prompt: false,
                });
            }
            "--freeze" => {
                mode = Some(Command::Freeze {
                    combo: String::new(),
                    max_secs: 0,
                    debug_sleep_ms: None,
                });
            }
            "--json" => json = true,
            "--prompt" => prompt = true,
            "--combo" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --combo".to_string())?;
                combo = Some(next.clone());
                i += 1;
            }
            "--max-secs" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --max-secs".to_string())?;
                let secs = parse_u64(next)?;
                max_secs = Some(secs);
                i += 1;
            }
            "--debug-sleep-in-callback-ms" => {
                let next = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --debug-sleep-in-callback-ms".to_string())?;
                debug_sleep_ms = Some(parse_u64(next)?);
                i += 1;
            }
            _ => {
                return Err(format!("unknown argument: {}", args[i]));
            }
        }
        i += 1;
    }

    match mode {
        Some(Command::CheckPermissions { .. }) => {
            if !json {
                return Err("--check-permissions requires --json".to_string());
            }
            Ok(Command::CheckPermissions { json: true, prompt })
        }
        Some(Command::Freeze { .. }) => {
            let combo = combo.ok_or_else(|| "--freeze requires --combo".to_string())?;
            let max_secs = max_secs.ok_or_else(|| "--freeze requires --max-secs".to_string())?;
            let parsed = combo::parse_combo(&combo).map_err(|e| e)?;
            let combo = parsed.canonical;
            Ok(Command::Freeze {
                combo,
                max_secs,
                debug_sleep_ms,
            })
        }
        None => Err("one of --check-permissions or --freeze is required".to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
fn unsupported() -> ! {
    eprintln!("vigil-lock-helper: unsupported platform");
    std::process::exit(EXIT_UNSUPPORTED);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    match parse_args(env::args().skip(1).collect()) {
        Ok(command) => match command {
            Command::CheckPermissions { .. } | Command::Freeze { .. } => unsupported(),
        },
        Err(err) => {
            eprintln!("error: {err}");
            print_usage();
            std::process::exit(EXIT_INVALID_ARGS);
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let command = match parse_args(env::args().skip(1).collect()) {
        Ok(cmd) => cmd,
        Err(err) => {
            eprintln!("error: {err}");
            print_usage();
            std::process::exit(EXIT_INVALID_ARGS);
        }
    };

    let status = match command {
        Command::CheckPermissions { json: _, prompt } => {
            let report = macos::check_permissions(prompt);
            println!("{}", report);
            if report.ready() {
                EXIT_OK
            } else {
                EXIT_PERMISSION_FAIL
            }
        }
        Command::Freeze {
            combo,
            max_secs,
            debug_sleep_ms,
        } => match macos::freeze(&combo, max_secs, debug_sleep_ms) {
            Ok(()) => EXIT_OK,
            Err(code) => code,
        },
    };
    std::process::exit(status);
}
