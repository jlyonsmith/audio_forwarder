use audio_forwarder::{AudioForwarder, StreamConfig};
use clap::{Parser, Subcommand};
use env_logger::Env;
use log::{error, LevelFilter};
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
struct AudioForwarderArgs {
    /// Command to execute
    #[command(subcommand)]
    command: Commands,

    /// Set the logging level (e.g., info, debug, trace)
    #[arg(long, default_value_t = LevelFilter::Info)]
    log_level: LevelFilter,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List available audio hosts, devices and stream configurations
    List,
    /// List a remote computer's audio hosts, devices and stream configurations
    ListRemote(ListRemoteArgs),
    /// Receive audio from a remote computer
    Receive(ReceiveArgs),
    /// Send audio to a remote computer
    Send(SendArgs),
    /// Listen for incoming audio connections from remote computers
    Listen(ListenArgs),
}

#[derive(Parser, Debug)]
pub struct ReceiveArgs {
    /// The name of the local audio device host to send audio to
    #[arg(long = "host")]
    pub host_name: String,

    /// The name of the local audio device to send audio to
    #[arg(long = "device")]
    pub device_name: Option<String>,

    /// The local audio device stream configuration in the format "<channels>x<sample_rate>" to send audio to
    #[arg(long = "config", value_parser = StreamConfig::parse)]
    pub stream_config: Option<StreamConfig>,

    /// The IP address and port of a remote computer to receive audio from
    #[arg(long = "addr")]
    pub sock_addr: SocketAddr,
}

#[derive(Parser, Debug)]
struct SendArgs {
    /// The name of the audio host to receive audio from
    #[arg(long = "host")]
    host_name: String,

    /// The name of the audio device to receive audio from
    #[arg(long = "device")]
    device_name: Option<String>,

    /// The audio device stream configuration in the format "<channels>x<sample_rate>" to receive audio from
    #[arg(long = "config", value_parser = StreamConfig::parse)]
    stream_config: Option<StreamConfig>,

    /// The IP address and port to send audio to
    #[arg(long = "addr")]
    sock_addr: SocketAddr,
}

#[derive(Parser, Debug)]
struct ListenArgs {
    /// The IP address and port to listen on for incoming requests
    #[arg(long = "addr")]
    sock_addr: SocketAddr,
}

#[derive(Parser, Debug)]
struct ListRemoteArgs {
    /// The IP address and port of the computer to list remote audio devices from
    #[arg(long = "addr")]
    sock_addr: SocketAddr,
}

#[tokio::main]
async fn main() {
    let args = match AudioForwarderArgs::try_parse_from(std::env::args_os()) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("{}", err.to_string());
            std::process::exit(1)
        }
    };

    let tool = AudioForwarder::new();
    let log_level = args.log_level;

    fn init_logger(log_level: LevelFilter) {
        env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", log_level.to_string()))
            .init();
    }

    let result = match args.command {
        Commands::Receive(receive_args) => {
            init_logger(log_level);
            tool.receive(
                &receive_args.sock_addr,
                &receive_args.host_name,
                &receive_args.device_name,
                &receive_args.stream_config,
            )
            .await
        }
        Commands::Send(send_args) => {
            init_logger(log_level);
            tool.send(
                &send_args.sock_addr,
                &send_args.host_name,
                &send_args.device_name,
                &send_args.stream_config,
            )
            .await
        }
        Commands::List => tool.list(),
        Commands::ListRemote(list_remote_args) => {
            tool.list_remote(&list_remote_args.sock_addr).await
        }
        Commands::Listen(listen_args) => {
            init_logger(log_level);
            tool.listen(&listen_args.sock_addr).await
        }
    };

    match result {
        Ok(_) => std::process::exit(0),
        Err(err) => {
            error!("{}", err);
            std::process::exit(1)
        }
    }
}
