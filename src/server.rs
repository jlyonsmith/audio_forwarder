pub use crate::{audio_caps::AudioCaps, messages::NetworkMessage, udp_server::UdpServer};
use crate::{DeviceConfig, StreamConfig, SERVER_TIMEOUT};
use anyhow::Context;
use env_logger::Env;
use futures::{SinkExt, StreamExt};
use log::{error, info, LevelFilter};
use rmp_serde::to_vec;
use std::{
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    runtime::Runtime,
    select, signal,
    task::JoinHandle,
    time::timeout,
};
use tokio_util::{
    codec::{Framed, LengthDelimitedCodec},
    sync::CancellationToken,
};

pub struct Server {
    handle_map:
        Arc<Mutex<std::collections::HashMap<DeviceConfig, (CancellationToken, JoinHandle<()>)>>>,
}

impl Server {
    pub fn new(log_level: LevelFilter) -> Server {
        env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", log_level.to_string()))
            .init();
        Server {
            handle_map: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    async fn handle_connection(&self, socket: &mut TcpStream) -> anyhow::Result<()> {
        let remote_addr = socket.peer_addr()?;
        let mut framed = Framed::new(&mut *socket, LengthDelimitedCodec::new());

        // Process just one message from the stream
        let frame = match timeout(SERVER_TIMEOUT, framed.next()).await? {
            Some(result) => result?,
            None => {
                return Err(anyhow::anyhow!(
                    "Timeout waiting for message from {}",
                    remote_addr
                ));
            }
        };

        // Deserialize the received bytes
        let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

        // Match on the deserialized enum
        match message {
            NetworkMessage::List => {
                info!("Handling list remote message from {}", remote_addr);
                let output = AudioCaps::get_device_list_string().unwrap();
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

                let (input_device, input_device_cfg, buffer_frames) = AudioCaps::get_input_device(
                    &host,
                    &device,
                    &config.and_then(|s| StreamConfig::from_str(&s).ok()),
                )?;

                {
                    let mut locked_map = self.handle_map.lock().unwrap();

                    if let Some((cancel_token, handle)) = locked_map.remove(&input_device_cfg) {
                        info!(
                            "Stopping existing stream for input device {}",
                            &input_device_cfg
                        );
                        cancel_token.cancel();
                        handle.await.ok();
                    }
                }

                let local_udp_socket = UdpSocket::bind("0.0.0.0:0")
                    .await
                    .context("Failed to bind UDP socket")?;
                let remote_udp_addr = SocketAddr::from_str(&udp_addr)?;

                info!(
                    "Receiving local input audio device {} and sending to udp://{}",
                    &input_device_cfg.host_name, remote_udp_addr
                );

                let message = NetworkMessage::SendAudioResponse {
                    actual_host: host,
                    actual_device: input_device_cfg.device_name.to_owned(),
                    actual_config: input_device_cfg.stream_cfg.to_string(),
                };

                framed.send(to_vec(&message)?.into()).await?;

                let cancel_token = CancellationToken::new();
                let cancel_token_clone = cancel_token.clone();
                let input_device_cfg_clone = input_device_cfg.clone();
                let handle = tokio::task::spawn_blocking(move || {
                    // cpal::Stream is !Send so we must use a dedicated thread for the UdpServer
                    let rt = Runtime::new().unwrap();
                    rt.block_on(async {
                        UdpServer::send_audio(
                            local_udp_socket,
                            remote_udp_addr,
                            input_device,
                            input_device_cfg_clone.stream_cfg,
                            buffer_frames,
                            cancel_token_clone,
                        )
                        .await
                        .ok();
                    });
                });

                {
                    let mut locked_map = self.handle_map.lock().unwrap();

                    locked_map.insert(input_device_cfg, (cancel_token, handle));
                }

                return Ok(());
            }
            NetworkMessage::ReceiveAudio {
                host,
                device,
                config,
            } => {
                info!("Handling receive audio from remote message");

                let (output_device, output_device_cfg, buffer_frames) =
                    AudioCaps::get_output_device(
                        &host,
                        &device,
                        &config.and_then(|s| StreamConfig::from_str(&s).ok()),
                    )?;

                {
                    let mut locked_map = self.handle_map.lock().unwrap();

                    if let Some((cancel_token, handle)) = locked_map.remove(&output_device_cfg) {
                        info!(
                            "Stopping existing stream for output device {}",
                            &output_device_cfg
                        );
                        cancel_token.cancel();
                        handle.await.ok();
                    }
                }

                let local_udp_socket = UdpSocket::bind("0.0.0.0:0")
                    .await
                    .context("Failed to bind UDP socket")?;

                info!(
                    "Receiving audio on udp://{} and sending to local output device {}",
                    local_udp_socket.local_addr()?,
                    output_device_cfg,
                );

                let message = NetworkMessage::SendAudioResponse {
                    actual_host: host,
                    actual_device: output_device_cfg.device_name.to_owned(),
                    actual_config: output_device_cfg.stream_cfg.to_string(),
                };

                framed.send(to_vec(&message)?.into()).await?;

                let cancel_token = CancellationToken::new();
                let cancel_token_clone = cancel_token.clone();
                let output_device_cfg_clone = output_device_cfg.clone();
                let handle = tokio::task::spawn_blocking(move || {
                    // cpal::Stream is !Send so we must use a dedicated thread for the UdpServer
                    let rt = Runtime::new().unwrap();
                    rt.block_on(async {
                        UdpServer::receive_audio(
                            local_udp_socket,
                            output_device,
                            output_device_cfg_clone.stream_cfg,
                            buffer_frames,
                            cancel_token_clone,
                        )
                        .await
                        .ok();
                    });
                });

                {
                    let mut locked_map = self.handle_map.lock().unwrap();

                    locked_map.insert(output_device_cfg, (cancel_token, handle));
                }

                return Ok(());
            }
            _ => {
                error!("Unexpected message");
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
                        Err(e) => error!("Error handling connection - {}", e),
                    }
                }
                _ = signal::ctrl_c() => {
                    info!("Stopping server");
                    break;
                }
            }
        }

        Ok(())
    }
}
