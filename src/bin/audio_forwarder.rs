use audio_forwarder::{error, AudioForwarderLog, AudioForwarderTool};
use core::fmt::Arguments;
use std::sync::Arc;
use termion::color;

struct AudioForwarderLogger;

impl AudioForwarderLogger {
    fn new() -> AudioForwarderLogger {
        AudioForwarderLogger {}
    }
}

impl AudioForwarderLog for AudioForwarderLogger {
    fn output(self: &Self, args: Arguments) {
        println!("{}", args);
    }
    fn warning(self: &Self, args: Arguments) {
        eprintln!("{}warning: {}", color::Fg(color::Yellow), args);
    }
    fn error(self: &Self, args: Arguments) {
        eprintln!("{}error: {}", color::Fg(color::Red), args);
    }
}

fn main() {
    let logger = Arc::new(AudioForwarderLogger::new());

    if let Err(error) = AudioForwarderTool::new(logger.clone()).run(std::env::args_os()) {
        error!(logger, "{}", error);
        std::process::exit(1);
    }
}
