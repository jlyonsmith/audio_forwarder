pub use crate::{audio_caps::AudioCaps, messages::NetworkMessage, udp_server::UdpServer};
use crate::{DeviceConfig, StreamConfig, SERVER_TIMEOUT};
use anyhow::Context;
use env_logger::Env;
use futures::{future::join_all, SinkExt, StreamExt};
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

struct ConnectionInfo {
    cancel_token: CancellationToken,
    udp_task_handle: JoinHandle<()>,
    socket_task_handle: JoinHandle<()>,
}

pub struct Server {
    handle_map: Arc<Mutex<std::collections::HashMap<DeviceConfig, ConnectionInfo>>>,
}

impl Server {
    pub fn new(log_level: LevelFilter) -> Server {
        env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", log_level.to_string()))
            .init();
        Server {
            handle_map: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    async fn handle_connection(
        &self,
        stream: TcpStream,
        remote_addr: SocketAddr,
    ) -> anyhow::Result<()> {
        let mut framed_stream = Framed::new(stream, LengthDelimitedCodec::new());

        // Process just one message from the stream
        let frame = match timeout(SERVER_TIMEOUT, framed_stream.next()).await? {
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

                framed_stream
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

                    if let Some(info) = locked_map.remove(&input_device_cfg) {
                        info!(
                            "Stopping existing stream for input device {}",
                            &input_device_cfg
                        );
                        info.cancel_token.cancel();
                        join_all([info.udp_task_handle, info.socket_task_handle]).await;
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

                framed_stream.send(to_vec(&message)?.into()).await?;

                let cancel_token = CancellationToken::new();
                let cancel_token_clone = cancel_token.clone();
                let input_device_cfg_clone = input_device_cfg.clone();
                let udp_task_handle = tokio::task::spawn_blocking(move || {
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

                let cancel_token_clone = cancel_token.clone();
                let socket_task_handle = tokio::spawn(async move {
                    loop {
                        select! {
                            _ = cancel_token_clone.cancelled() => {
                                break;
                            }
                            Some(Ok(bytes)) = framed_stream.next() => {
                                if bytes.len() == 0 {
                                    cancel_token_clone.cancel();
                                    break;
                                }
                            }
                        }

                        info!("Close TCP connection to {}", remote_addr);
                    }
                });

                {
                    let mut locked_map = self.handle_map.lock().unwrap();

                    locked_map.insert(
                        input_device_cfg,
                        ConnectionInfo {
                            cancel_token,
                            udp_task_handle,
                            socket_task_handle,
                        },
                    );
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

                    if let Some(info) = locked_map.remove(&output_device_cfg) {
                        info!(
                            "Stopping existing stream for output device {}",
                            &output_device_cfg
                        );
                        info.cancel_token.cancel();
                        join_all([info.udp_task_handle, info.socket_task_handle]).await;
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

                framed_stream.send(to_vec(&message)?.into()).await?;

                let cancel_token = CancellationToken::new();
                let cancel_token_clone = cancel_token.clone();
                let output_device_cfg_clone = output_device_cfg.clone();
                let udp_task_handle = tokio::task::spawn_blocking(move || {
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

                let cancel_token_clone = cancel_token.clone();
                let socket_task_handle = tokio::spawn(async move {
                    // Keep the TCP connection alive until cancelled
                    loop {
                        select! {
                            _ = cancel_token_clone.cancelled() => {
                                break;
                            }
                            Some(Ok(bytes)) = framed_stream.next() => {
                                if bytes.len() == 0 {
                                    cancel_token_clone.cancel();
                                    break;
                                }
                            }
                        }

                        info!("Close TCP connection to {}", remote_addr);
                    }
                });

                {
                    let mut locked_map = self.handle_map.lock().unwrap();

                    locked_map.insert(
                        output_device_cfg,
                        ConnectionInfo {
                            cancel_token,
                            udp_task_handle,
                            socket_task_handle,
                        },
                    );
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
                Ok((stream, addr)) = listener.accept() => {
                    info!("Accepted connection from {}", addr);

                    match self.handle_connection(stream, addr).await {
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

        let mut handles = Vec::new();

        {
            let mut locked_map = self.handle_map.lock().unwrap();

            for (device_cfg, info) in locked_map.drain() {
                info!("Stopping stream for device {}", &device_cfg);
                info.cancel_token.cancel();
                handles.push(info.udp_task_handle);
                handles.push(info.socket_task_handle);
            }
        }

        join_all(handles.into_iter()).await;

        Ok(())
    }
}
