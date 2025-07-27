mod log_macros;

use anyhow::Context;
use clap::Parser;
use core::fmt::Arguments;
use std::{
    error::Error,
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
};

pub trait AudioForwarderLog {
    fn output(self: &Self, args: Arguments);
    fn warning(self: &Self, args: Arguments);
    fn error(self: &Self, args: Arguments);
}

pub struct AudioForwarderTool<'a> {
    log: &'a dyn AudioForwarderLog,
}

#[derive(Parser)]
#[clap(version, about, long_about = None)]
struct Cli {
    /// Disable colors in output
    #[arg(long = "no-color", short = 'n', env = "NO_CLI_COLOR")]
    no_color: bool,

    /// The input file
    #[arg(value_name = "INPUT_FILE")]
    input_file: Option<PathBuf>,

    /// The output file
    #[arg(value_name = "OUTPUT_FILE")]
    output_file: Option<PathBuf>,
}

impl Cli {
    fn get_output(&self) -> anyhow::Result<Box<dyn Write>> {
        match self.output_file {
            Some(ref path) => File::create(path)
                .context(format!(
                    "Unable to create file '{}'",
                    path.to_string_lossy()
                ))
                .map(|f| Box::new(f) as Box<dyn Write>),
            None => Ok(Box::new(io::stdout())),
        }
    }

    fn get_input(&self) -> anyhow::Result<Box<dyn Read>> {
        match self.input_file {
            Some(ref path) => File::open(path)
                .context(format!("Unable to open file '{}'", path.to_string_lossy()))
                .map(|f| Box::new(f) as Box<dyn Read>),
            None => Ok(Box::new(io::stdin())),
        }
    }
}

impl<'a> AudioForwarderTool<'a> {
    pub fn new(log: &'a dyn AudioForwarderLog) -> AudioForwarderTool<'a> {
        AudioForwarderTool { log }
    }

    pub fn run(
        self: &mut Self,
        args: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> Result<(), Box<dyn Error>> {
        let cli = match Cli::try_parse_from(args) {
            Ok(m) => m,
            Err(err) => {
                output!(self.log, "{}", err.to_string());
                return Ok(());
            }
        };

        let mut content = String::new();

        cli.get_input()?.read_to_string(&mut content)?;

        write!(cli.get_output()?, "{}", content)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_test() {
        struct TestLogger;

        impl TestLogger {
            fn new() -> TestLogger {
                TestLogger {}
            }
        }

        impl AudioForwarderLog for TestLogger {
            fn output(self: &Self, _args: Arguments) {}
            fn warning(self: &Self, _args: Arguments) {}
            fn error(self: &Self, _args: Arguments) {}
        }

        let logger = TestLogger::new();
        let mut tool = AudioForwarderTool::new(&logger);
        let args: Vec<std::ffi::OsString> = vec!["".into(), "--help".into()];

        tool.run(args).unwrap();
    }
}
