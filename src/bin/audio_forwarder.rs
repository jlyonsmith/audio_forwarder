use audio_forwarder::{AudioForwarderArgs, AudioForwarderTool};
use clap::Parser;
use log::error;

#[tokio::main]
async fn main() {
    // Parse command-line arguments and send errors to stderr
    let args = match AudioForwarderArgs::try_parse_from(std::env::args_os()) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("{}", err.to_string());
            std::process::exit(1)
        }
    };

    // Now we can initialize the logger
    env_logger::init();

    if let Err(error) = AudioForwarderTool::new().run(&args).await {
        error!("{}", error);
        std::process::exit(1);
    }

    std::process::exit(0);
}
