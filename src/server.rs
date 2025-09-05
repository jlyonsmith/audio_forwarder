use crate::StreamConfig;
pub use crate::{audio_caps::AudioCaps, messages::NetworkMessage, udp_server::UdpServer};
use anyhow::Context;
use cpal::traits::DeviceTrait;
use env_logger::Env;
use futures::{SinkExt, StreamExt};
use log::{error, info, LevelFilter};
use rmp_serde::to_vec;
use std::{net::SocketAddr, str::FromStr};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    select, signal,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub struct Server {}

impl Server {
    pub fn new(log_level: LevelFilter) -> Server {
        env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", log_level.to_string()))
            .init();
        Server {}
    }

    async fn handle_connection(&self, socket: &mut TcpStream) -> anyhow::Result<()> {
        let remote_addr = socket.peer_addr()?;
        let mut framed = Framed::new(&mut *socket, LengthDelimitedCodec::new());

        // TODO @john: Process just one message from the stream
        while let Some(result) = framed.next().await {
            let frame = result?;

            // Deserialize the received bytes
            let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

            // Match on the deserialized enum
            match message {
                NetworkMessage::List => {
                    info!("Handling list remote message from {}", remote_addr);
                    let output = AudioCaps::list_to_string().unwrap();
                    let list_remote_response_message = NetworkMessage::ListResponse { output };

                    framed
                        .send(to_vec(&list_remote_response_message).unwrap().into())
                        .await?;
                }
                NetworkMessage::SendAudio {
                    host,
                    device,
                    config,
                    udp_addr,
                } => {
                    info!("Handling send audio remote message from {}", remote_addr);

                    let stream_config = config.and_then(|s| StreamConfig::from_str(&s).ok());
                    let (actual_device, actual_config) =
                        AudioCaps::get_input_device_config(&host, &device, &stream_config)?;
                    let local_udp_socket = UdpSocket::bind("0.0.0.0:0")
                        .await
                        .context("Failed to bind UDP socket")?;
                    let remote_udp_addr = SocketAddr::from_str(&udp_addr)?;

                    info!(
                        "Receiving local input audio device {} -> {} -> {} and sending to udp://{}",
                        host,
                        actual_device.name().unwrap(),
                        StreamConfig::to_config_string(&actual_config),
                        remote_udp_addr
                    );

                    let message = NetworkMessage::SendAudioResponse {
                        actual_host: host,
                        actual_device: actual_device.name().unwrap(),
                        actual_config: StreamConfig::to_config_string(&actual_config),
                    };

                    framed.send(to_vec(&message)?.into()).await?;

                    UdpServer::send_audio(
                        local_udp_socket,
                        remote_udp_addr,
                        actual_device,
                        actual_config,
                    )
                    .await?;
                }
                NetworkMessage::ReceiveAudio {
                    host,
                    device,
                    config,
                } => {
                    info!("Handling receive audio from remote message");

                    let config = config.and_then(|s| StreamConfig::from_str(&s).ok());
                    let (actual_device, actual_config) =
                        AudioCaps::get_output_device_config(&host, &device, &config)?;
                    let local_udp_socket = UdpSocket::bind("0.0.0.0:0")
                        .await
                        .context("Failed to bind UDP socket")?;

                    info!(
                        "Receiving audio on udp://{} and sending to local output device {} -> {} -> {}",
                        local_udp_socket.local_addr()?,
                        host,
                        actual_device.name().unwrap(),
                        StreamConfig::to_config_string(&actual_config),
                    );

                    let message = NetworkMessage::SendAudioResponse {
                        actual_host: host,
                        actual_device: actual_device.name().unwrap(),
                        actual_config: StreamConfig::to_config_string(&actual_config),
                    };

                    framed.send(to_vec(&message)?.into()).await?;

                    UdpServer::receive_audio(local_udp_socket, actual_device, actual_config)
                        .await
                        .ok();
                }
                _ => {
                    error!("Unexpected message");
                }
            }
        }

        Ok(())
    }

    pub async fn listen(&self, sock_addr: &SocketAddr) -> anyhow::Result<()> {
        // Bind the listener to an address
        let listener = TcpListener::bind(sock_addr).await?;

        info!("Server listening on {}", sock_addr);

        loop {
            select! {
                Ok((mut socket, addr)) = listener.accept() => {
                    info!("Accepted connection from {}", addr);
                    match self.handle_connection(&mut socket).await {
                        Ok(_) => {}
                        Err(e) => error!("Error handling connection: {}", e),
                    }
                }
                _ = signal::ctrl_c() => {
                    // TODO @john: Create a signal that can be used to stop any running tasks
                    info!("Stopping server");
                    break;
                }
            }
        }

        Ok(())
    }
}
