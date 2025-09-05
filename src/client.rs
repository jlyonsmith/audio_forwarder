use crate::{
    audio_caps::AudioCaps, messages::NetworkMessage, stream_config::StreamConfig,
    udp_server::UdpServer,
};
use anyhow::{bail, Context};
use cpal::traits::DeviceTrait;
use env_logger::Env;
use futures::{SinkExt, StreamExt};
use log::{info, LevelFilter};
use rmp_serde::to_vec;
use std::net::SocketAddr;
use tokio::{
    net::{TcpStream, UdpSocket},
    time::timeout,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

// TODO @john: Make this configurable
pub struct Client {}

impl Client {
    pub fn new(log_level: LevelFilter) -> Client {
        env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", log_level.to_string()))
            .init();
        Client {}
    }

    pub fn list() -> anyhow::Result<()> {
        println!("{}", AudioCaps::list_to_string()?);
        Ok(())
    }

    pub async fn list_remote(sock_addr: &SocketAddr) -> Result<(), anyhow::Error> {
        let socket = TcpStream::connect(sock_addr).await?;
        let mut framed = Framed::new(socket, LengthDelimitedCodec::new());
        let list_remote_message = NetworkMessage::List;

        framed.send(to_vec(&list_remote_message)?.into()).await?;

        if let Some(result) = framed.next().await {
            let frame = result?;
            let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

            match message {
                NetworkMessage::ListResponse { output } => {
                    print!("{}", output);
                }
                _ => bail!("Unexpected response from remote"),
            }
        }

        Ok(())
    }

    pub async fn receive(
        &self,
        sock_addr: &SocketAddr,
        output_host: &str,
        output_device: &Option<String>,
        output_stream_config: &Option<StreamConfig>,
        input_host: &str,
        input_device: &Option<String>,
        input_stream_config: &Option<StreamConfig>,
    ) -> anyhow::Result<()> {
        let (actual_output_device, actual_output_config) =
            AudioCaps::get_output_device_config(output_host, output_device, output_stream_config)?;
        let socket = TcpStream::connect(sock_addr).await?;
        let udp_socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("Failed to bind UDP socket")?;
        let mut framed = Framed::new(socket, LengthDelimitedCodec::new());
        let message = NetworkMessage::SendAudio {
            host: input_host.to_string(),
            device: input_device.clone(),
            config: input_stream_config.as_ref().map(|x| x.to_string()),
            udp_addr: udp_socket.local_addr()?.to_string(),
        };

        framed.send(to_vec(&message)?.into()).await?;

        if let Some(result) = timeout(crate::SERVER_TIMEOUT, framed.next())
            .await
            .context("Timed out waiting for response from server")?
        {
            let frame = result?;
            let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

            match message {
                NetworkMessage::SendAudioResponse {
                    actual_host: actual_remote_host,
                    actual_device: actual_remote_device,
                    actual_config: actual_remote_config,
                } => {
                    info!(
                        "Capturing remote input audio from {} -> {} -> {}, receiving on udp://{} and forwarding to local output device {} -> {} -> {}",
                        actual_remote_host,
                        actual_remote_device,
                        actual_remote_config,
                        udp_socket.local_addr()?.to_string(),
                        output_host,
                        actual_output_device.name().unwrap_or("Unknown".to_string()),
                        StreamConfig::to_config_string(&actual_output_config),
                    );

                    UdpServer::receive_audio(
                        udp_socket,
                        actual_output_device,
                        actual_output_config,
                    )
                    .await?;
                }
                _ => bail!("Unexpected response from remote"),
            }
        }

        Ok(())
    }

    pub async fn send(
        &self,
        sock_addr: &SocketAddr,
        input_host: &str,
        input_device: &Option<String>,
        input_config: &Option<StreamConfig>,
        output_host: &str,
        output_device: &Option<String>,
        output_config: &Option<StreamConfig>,
    ) -> anyhow::Result<()> {
        let (actual_input_device, actual_input_config) =
            AudioCaps::get_input_device_config(input_host, input_device, input_config)?;
        let socket = TcpStream::connect(sock_addr).await?;
        let udp_socket = UdpSocket::bind("0.0.0.0:0").await?;
        let mut framed = Framed::new(socket, LengthDelimitedCodec::new());
        let message = NetworkMessage::ReceiveAudio {
            host: output_host.to_string(),
            device: output_device.clone(),
            config: output_config.as_ref().map(|x| x.to_string()),
        };

        framed.send(to_vec(&message)?.into()).await?;

        if let Some(result) = timeout(crate::SERVER_TIMEOUT, framed.next())
            .await
            .context("Timed out waiting for response from server")?
        {
            let frame = result?;
            let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

            match message {
                NetworkMessage::ReceiveAudioResponse {
                    actual_host: actual_remote_host,
                    actual_device: actual_remote_device,
                    actual_config: actual_remote_config,
                    udp_addr,
                } => {
                    let actual_remote_config: StreamConfig = actual_remote_config.parse()?;
                    let remote_udp_addr: SocketAddr = udp_addr.parse()?;

                    info!(
                        "Capturing local input audio from {} -> {} -> {}, sending to udp://{} then forwarding to remote output audio {} -> {} -> {}",
                        input_host,
                        actual_input_device.name().unwrap(),
                        StreamConfig::to_config_string(&actual_input_config),
                        remote_udp_addr,
                        actual_remote_host,
                        actual_remote_device,
                        actual_remote_config.to_string()
                    );

                    UdpServer::send_audio(
                        udp_socket,
                        remote_udp_addr,
                        actual_input_device,
                        actual_input_config,
                    )
                    .await?;
                }
                _ => bail!("Unexpected response from remote"),
            }
        }

        Ok(())
    }
}
