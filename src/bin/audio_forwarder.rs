use audio_forwarder::{Client, Server, StreamConfig};
use clap::{Parser, Subcommand};
use log::{error, LevelFilter};
use std::{net::SocketAddr, str::FromStr};

#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
struct AudioForwarderArgs {
    /// Command to execute
    #[command(subcommand)]
    command: Commands,

    /// Set the logging level (e.g., info, debug, trace)
    #[arg(long = "log-level", default_value_t = LevelFilter::Info)]
    log_level: LevelFilter,

    /// Enable metrics server
    #[arg(long = "metrics-addr")]
    metrics_addr: Option<SocketAddr>,
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
    #[arg(long = "local-host")]
    pub local_host: String,

    /// The name of the local audio device to send audio to
    #[arg(long = "local-device")]
    pub local_device: Option<String>,

    /// The local audio device stream configuration in the format "<channels>x<sample_rate>" to send audio to
    #[arg(long = "local-stream-cfg", value_parser = StreamConfig::from_str)]
    pub local_config: Option<StreamConfig>,

    /// The name of the audio host to receive audio from on the remote computer
    #[arg(long = "remote-host")]
    remote_host: String,

    /// The name of the audio device to receive audio from on the remote computer
    #[arg(long = "remote-device")]
    remote_device: Option<String>,

    /// The audio device stream configuration in the format "<channels>x<sample_rate>" to receive audio from on the remote computer
    #[arg(long = "remote-stream-cfg", value_parser = StreamConfig::from_str)]
    remote_config: Option<StreamConfig>,

    /// The IP address and port of a remote computer to receive audio from
    #[arg(long = "addr")]
    pub sock_addr: SocketAddr,
}

#[derive(Parser, Debug)]
struct SendArgs {
    /// The name of the audio host to receive audio from
    #[arg(long = "local-host")]
    local_host: String,

    /// The name of the audio device to receive audio from
    #[arg(long = "local-device")]
    local_device: Option<String>,

    /// The audio device stream configuration in the format "<channels>x<sample_rate>" to receive audio from
    #[arg(long = "local-stream-cfg", value_parser = StreamConfig::from_str)]
    local_config: Option<StreamConfig>,

    /// The name of the audio host to send audio to on the remote computer
    #[arg(long = "remote-host")]
    remote_host: String,

    /// The name of the audio device to send audio to on the remote computer
    #[arg(long = "remote-device")]
    remote_device: Option<String>,

    /// The audio device stream configuration in the format "<channels>x<sample_rate>" to send audio to on the remote computer
    #[arg(long = "remote-stream-cfg", value_parser = StreamConfig::from_str)]
    remote_config: Option<StreamConfig>,

    /// The IP address and port of a remote computer to send audio to
    #[arg(long = "addr")]
    sock_addr: SocketAddr,

    /// If there is a firewall between the client and server, specify the firewall address and port to use for UDP traffic
    #[arg(long = "firewall-udp-addr")]
    firewall_udp_addr: Option<SocketAddr>,
}

#[derive(Parser, Debug)]
struct ListenArgs {
    /// The IP address and port to listen on for incoming requests from clients
    #[arg(long = "addr")]
    sock_addr: SocketAddr,
}

#[derive(Parser, Debug)]
struct ListRemoteArgs {
    /// The IP address and port of the server running on the remote computer to list remote audio devices from
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
    let log_level = args.log_level;
    let metrics_addr = args.metrics_addr;
    let result = match args.command {
        Commands::Receive(receive_args) => {
            Client::new(log_level, metrics_addr)
                .receive(
                    &receive_args.sock_addr,
                    &receive_args.local_host,
                    &receive_args.local_device,
                    &receive_args.local_config,
                    &receive_args.remote_host,
                    &receive_args.remote_device,
                    &receive_args.remote_config,
                )
                .await
        }
        Commands::Send(send_args) => {
            Client::new(log_level, metrics_addr)
                .send(
                    &send_args.sock_addr,
                    &send_args.local_host,
                    &send_args.local_device,
                    &send_args.local_config,
                    &send_args.remote_host,
                    &send_args.remote_device,
                    &send_args.remote_config,
                    &send_args.firewall_udp_addr,
                )
                .await
        }
        Commands::List => Client::list(),
        Commands::ListRemote(list_remote_args) => {
            Client::list_remote(&list_remote_args.sock_addr).await
        }
        Commands::Listen(listen_args) => {
            Server::new(log_level, metrics_addr)
                .listen(&listen_args.sock_addr)
                .await
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
