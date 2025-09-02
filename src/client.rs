use crate::{
    audio_caps::AudioCaps, messages::NetworkMessage, stream_config::StreamConfig,
    udp_server::UdpServer,
};
use anyhow::{bail, Context};
use cpal::{traits::DeviceTrait, SampleFormat};
use env_logger::Env;
use futures::{SinkExt, StreamExt};
use log::{info, LevelFilter};
use rmp_serde::to_vec;
use std::net::SocketAddr;
use tokio::{net::TcpStream, time::timeout};
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
        host: &str,
        device: &Option<String>,
        stream_config: &Option<StreamConfig>,
    ) -> anyhow::Result<()> {
        let (device, config) = AudioCaps::get_output_device_config(host, device, stream_config)?;

        info!(
            "Listening at {} for requests to receive audio and forward to {} -> {} -> {} channel{} at {} Hz",
            sock_addr,
            host,
            device.name().unwrap_or("Unknown".to_string()),
            config.channels(),
            if config.channels() == 1 { "" } else { "s" },
            config.sample_rate().0,
        );

        let server = UdpServer::new();

        match config.sample_format() {
            SampleFormat::F32 => {
                server
                    .receive_audio::<f32>(sock_addr, &device, &config)
                    .await?
            }
            SampleFormat::I16 => {
                server
                    .receive_audio::<i16>(sock_addr, &device, &config)
                    .await?
            }
            SampleFormat::U16 => {
                server
                    .receive_audio::<u16>(sock_addr, &device, &config)
                    .await?
            }
            _ => panic!("Unsupported sample format on output device"),
        }

        Ok(())
    }

    pub async fn send(
        &self,
        sock_addr: &SocketAddr,
        input_host: &str,
        input_device: &Option<String>,
        input_stream_config: &Option<StreamConfig>,
        output_host: &str,
        output_device: &Option<String>,
        output_stream_config: &Option<StreamConfig>,
    ) -> anyhow::Result<()> {
        let (local_device, local_config) =
            AudioCaps::get_input_device_config(input_host, input_device, input_stream_config)?;

        info!(
            "Receiving local audio from input device {} -> {} -> {} channel{} at {} Hz",
            input_host,
            local_device.name().unwrap_or("Unknown".to_string()),
            local_config.channels(),
            if local_config.channels() == 1 {
                ""
            } else {
                "s"
            },
            local_config.sample_rate().0,
        );

        let socket = TcpStream::connect(sock_addr).await?;
        let mut framed = Framed::new(socket, LengthDelimitedCodec::new());
        let message = NetworkMessage::ReceiveAudio {
            host: output_host.to_string(),
            device: output_device.clone(),
            stream_config: output_stream_config.as_ref().map(|x| x.to_string()),
        };

        framed.send(to_vec(&message)?.into()).await?;

        if let Some(result) = timeout(crate::SERVER_TIMEOUT, framed.next())
            .await
            .context("Timed out waiting for response from server")?
        {
            let frame = result?;
            let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;
            let server = UdpServer::new();

            match message {
                NetworkMessage::ReceiveAudioResponse {
                    host: remote_host,
                    device: remote_device,
                    stream_config,
                    udp_addr: addr,
                } => {
                    let config: StreamConfig = stream_config.parse()?;
                    let udp_addr = addr.parse()?;

                    info!(
                        "Forwarding audio via {} to output device {} -> {} -> {} channel{} at {} Hz",
                        udp_addr,
                        remote_host,
                        remote_device,
                        config.channels,
                        if config.channels == 1 { "" } else { "s" },
                        config.sample_rate,
                    );

                    // TODO @john: Add matches for all the supported sample formats
                    match local_config.sample_format() {
                        SampleFormat::F32 => {
                            server
                                .send_audio::<f32>(&udp_addr, &local_device, &local_config)
                                .await?
                        }
                        SampleFormat::I16 => {
                            server
                                .send_audio::<i16>(&udp_addr, &local_device, &local_config)
                                .await?
                        }
                        SampleFormat::U16 => {
                            server
                                .send_audio::<u16>(&udp_addr, &local_device, &local_config)
                                .await?
                        }
                        _ => bail!("Unsupported sample format on input device"),
                    }
                }
                _ => bail!("Unexpected response from remote"),
            }
        }

        Ok(())
    }
}
