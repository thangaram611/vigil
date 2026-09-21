//! `vigil mode [auto|on|off]` — show or set the manual hold mode.
//!
//! Setting a mode only writes the mode file; the daemon applies it on its next
//! tick. With no argument, prints the bare mode word (scriptable).

use vigil::mode::{self, Mode};

use super::{die, load_config_or_exit};

pub fn run(requested: Option<Mode>) -> ! {
    let cfg = load_config_or_exit();
    let mode_file = std::path::Path::new(&cfg.mode_file);
    let Some(requested) = requested else {
        println!("{}", mode::read(mode_file).as_str());
        std::process::exit(0);
    };
    if let Err(e) = cfg
        .ensure_state_dir()
        .and_then(|()| mode::write(mode_file, requested))
    {
        die(&format!("could not set mode: {e}"));
    }
    anstream::println!("vigil: mode {} — {}", requested.as_str(), effect(requested));
    std::process::exit(0);
}

fn effect(mode: Mode) -> &'static str {
    match mode {
        Mode::Auto => "holding while agents are active",
        Mode::On => "holding regardless of agents (thermal and battery guards still apply)",
        Mode::Off => "never holding",
    }
}
