use crate::{
    audio_caps::AudioCaps, messages::NetworkMessage, stream_config::StreamConfig,
    udp_server::UdpServer, DeviceConfig, DeviceDirection, METRICS_CLIENT_SOCKADDR, SERVER_TIMEOUT,
};
use anyhow::{bail, Context};
use cpal::SampleFormat;
use env_logger::Env;
use futures::{SinkExt, StreamExt};
use log::{error, info, LevelFilter};
use metrics_exporter_tcp::TcpBuilder;
use rmp_serde::to_vec;
use std::net::SocketAddr;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpStream, UdpSocket},
    runtime::Runtime,
    select, signal,
    time::timeout,
};
use tokio_util::{
    codec::{Framed, LengthDelimitedCodec},
    sync::CancellationToken,
};

pub struct Client {}

impl Client {
    pub fn new(log_level: LevelFilter) -> Client {
        env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", log_level.to_string()))
            .init();
        Client {}
    }

    pub fn list_explanation() {
        println!("NOTE: The list of audio devices is filtered to:");
        println!();
        println!("- Input devices with mono or stereo channel support.");
        println!("- Output devices with stereo channel support.");
        println!("- A sample format of f32 or i32.");
        println!();
        println!("1 channel input devices will be upmixed to stereo when sending audio.");
        println!();
    }

    pub fn list() -> anyhow::Result<()> {
        Self::list_explanation();
        println!("{}", AudioCaps::get_device_list_string()?);
        Ok(())
    }

    pub async fn list_remote(remote_addr: &SocketAddr) -> Result<(), anyhow::Error> {
        let socket = TcpStream::connect(remote_addr).await?;
        let mut framed_stream = Framed::new(socket, LengthDelimitedCodec::new());
        let list_remote_message = NetworkMessage::List;

        framed_stream
            .send(to_vec(&list_remote_message)?.into())
            .await?;

        if let Some(result) = framed_stream.next().await {
            let frame = result?;
            let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

            match message {
                NetworkMessage::ListResponse { output } => {
                    Self::list_explanation();
                    print!("{}", output);
                }
                _ => bail!("Unexpected response from remote"),
            }
        }

        Ok(())
    }

    pub async fn receive(
        &self,
        remote_addr: &SocketAddr,
        output_host: &str,
        output_device: &Option<String>,
        output_stream_config: &Option<StreamConfig>,
        input_host: &str,
        input_device: &Option<String>,
        input_stream_config: &Option<StreamConfig>,
    ) -> anyhow::Result<()> {
        let metrics_builder = TcpBuilder::new().listen_address(METRICS_CLIENT_SOCKADDR);

        metrics_builder.install().context(format!(
            "Unable to start metrics collection on {}",
            METRICS_CLIENT_SOCKADDR
        ))?;

        let (output_device, output_device_cfg, buffer_frames) =
            AudioCaps::get_output_device(output_host, output_device, output_stream_config)?;
        let socket = TcpStream::connect(remote_addr).await?;
        let udp_socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("Failed to bind UDP socket")?;
        let mut framed_stream = Framed::new(socket, LengthDelimitedCodec::new());
        let message = NetworkMessage::SendAudio {
            host: input_host.to_string(),
            device: input_device.clone(),
            stream_cfg: input_stream_config.as_ref().map(|x| x.to_string()),
            udp_addr: udp_socket.local_addr()?.to_string(),
        };

        if output_device_cfg.stream_cfg.channels != 2
            || (output_device_cfg.stream_cfg.sample_format != SampleFormat::F32
                && output_device_cfg.stream_cfg.sample_format != SampleFormat::I32)
        {
            return Err(anyhow::anyhow!(
                "Local output device must be stereo and use f32 or i32 sample format"
            ));
        }

        info!("Actual local output device is {}", output_device_cfg);

        framed_stream.send(to_vec(&message)?.into()).await?;

        // Process a message with a timeout
        let frame = match timeout(SERVER_TIMEOUT, framed_stream.next()).await? {
            Some(result) => result?,
            None => {
                return Err(anyhow::anyhow!(
                    "Timeout waiting for message from tcp://{}",
                    remote_addr
                ));
            }
        };

        let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

        match message {
            NetworkMessage::SendAudioResponse {
                actual_host: actual_remote_host,
                actual_device: actual_remote_device,
                actual_stream_cfg: actual_remote_config,
            } => {
                let input_device_cfg = DeviceConfig {
                    direction: DeviceDirection::Input,
                    host_name: actual_remote_host,
                    device_name: actual_remote_device,
                    stream_cfg: actual_remote_config.parse()?,
                };

                if input_device_cfg.stream_cfg.channels > 2
                    || (input_device_cfg.stream_cfg.sample_format != SampleFormat::F32
                        && input_device_cfg.stream_cfg.sample_format != SampleFormat::I32)
                {
                    return Err(anyhow::anyhow!(
                        "Remote input device must be mono or stereo and use f32 or i32 sample format"
                    ));
                }

                info!("Actual local input device is {}", input_device_cfg,);

                let cancel_token = CancellationToken::new();
                let cancel_token_clone = cancel_token.clone();
                let mut udp_task_handle = tokio::task::spawn_blocking(move || {
                    // cpal::Stream is !Send so we must use a dedicated thread for the UdpServer
                    let rt = Runtime::new().unwrap();
                    rt.block_on(async {
                        UdpServer::receive_audio(
                            udp_socket,
                            output_device,
                            output_device_cfg,
                            buffer_frames,
                            cancel_token_clone,
                        )
                        .await
                        .ok();
                        // TODO(john): Output errors and device_id to channel
                    });
                });

                select! {
                    _ = framed_stream.next() => {
                        // Any data means the server closed the connection
                        cancel_token.cancel();
                    },
                    _ = &mut udp_task_handle => (),
                    _ = signal::ctrl_c() => {
                        cancel_token.cancel();
                    }
                }

                framed_stream.get_mut().shutdown().await?;
                udp_task_handle.await?;

                info!("Remote connection closed");
            }
            _ => {
                error!("Unexpected response from remote");
                framed_stream.get_mut().shutdown().await?;
            }
        }

        Ok(())
    }

    pub async fn send(
        &self,
        remote_addr: &SocketAddr,
        input_host: &str,
        input_device: &Option<String>,
        input_config: &Option<StreamConfig>,
        output_host: &str,
        output_device: &Option<String>,
        output_config: &Option<StreamConfig>,
    ) -> anyhow::Result<()> {
        let metrics_builder = TcpBuilder::new().listen_address(METRICS_CLIENT_SOCKADDR);

        metrics_builder.install().context(format!(
            "Unable to start metrics collection on {}",
            METRICS_CLIENT_SOCKADDR
        ))?;

        let (input_device, input_device_cfg, buffer_frames) =
            AudioCaps::get_input_device(input_host, input_device, input_config)?;
        let stream = TcpStream::connect(remote_addr).await?;
        let udp_socket = UdpSocket::bind("0.0.0.0:0").await?;
        let mut framed_stream = Framed::new(stream, LengthDelimitedCodec::new());
        let message = NetworkMessage::ReceiveAudio {
            host: output_host.to_string(),
            device: output_device.clone(),
            stream_cfg: output_config.as_ref().map(|x| x.to_string()),
        };

        if input_device_cfg.stream_cfg.channels > 2
            || (input_device_cfg.stream_cfg.sample_format != SampleFormat::F32
                && input_device_cfg.stream_cfg.sample_format != SampleFormat::I32)
        {
            return Err(anyhow::anyhow!(
                "Local input device must be mono or stereo and use f32 or i32 sample format"
            ));
        }

        info!("Actual local input device is {}", input_device_cfg);

        framed_stream.send(to_vec(&message)?.into()).await?;

        if let Some(result) = timeout(crate::SERVER_TIMEOUT, framed_stream.next())
            .await
            .context("Timed out waiting for response from server")?
        {
            let frame = result?;
            let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

            match message {
                NetworkMessage::ReceiveAudioResponse {
                    actual_host: actual_remote_host,
                    actual_device: actual_remote_device,
                    actual_stream_cfg: actual_remote_config,
                    udp_addr,
                } => {
                    let output_device_cfg = DeviceConfig {
                        direction: DeviceDirection::Output,
                        host_name: actual_remote_host,
                        device_name: actual_remote_device,
                        stream_cfg: actual_remote_config.parse()?,
                    };
                    let remote_udp_addr: SocketAddr = udp_addr.parse()?;

                    if output_device_cfg.stream_cfg.channels != 2
                        || (output_device_cfg.stream_cfg.sample_format != SampleFormat::F32
                            && output_device_cfg.stream_cfg.sample_format != SampleFormat::I32)
                    {
                        return Err(anyhow::anyhow!(
                            "Remote output device must be stereo and use f32 or i32 sample format"
                        ));
                    }

                    info!("Actual remote output device is {}", output_device_cfg);

                    let cancel_token = CancellationToken::new();
                    let cancel_token_clone = cancel_token.clone();
                    let mut udp_task_handle = tokio::task::spawn_blocking(move || {
                        // cpal::Stream is !Send so we must use a dedicated thread for the UdpServer
                        let rt = Runtime::new().unwrap();
                        rt.block_on(async {
                            UdpServer::send_audio(
                                udp_socket,
                                remote_udp_addr,
                                input_device,
                                input_device_cfg,
                                buffer_frames,
                                cancel_token_clone,
                            )
                            .await
                            .ok();
                            // TODO(john): Output errors and device_id to channel
                        });
                    });

                    select! {
                        _ = framed_stream.next() => {
                            // Any data means the server closed the connection
                            cancel_token.cancel();
                        },
                        _ = &mut udp_task_handle => (),
                        _ = signal::ctrl_c() => {
                            cancel_token.cancel();
                        }
                    }

                    framed_stream.get_mut().shutdown().await?;
                    udp_task_handle.await?;

                    info!("Remote connection closed");
                }
                _ => {
                    error!("Unexpected response from remote");
                    framed_stream.get_mut().shutdown().await?;
                }
            }
        }

        Ok(())
    }
}
