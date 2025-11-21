use crate::{
    audio_caps::AudioCaps, messages::NetworkMessage, udp_server::UdpServer, StreamConfig,
    SERVER_TIMEOUT,
};
use anyhow::Context;
use env_logger::Env;
use flume::Sender;
use futures::{future::join_all, SinkExt, StreamExt};
use log::{error, info, LevelFilter};
#[cfg(feature = "metrics")]
use metrics_exporter_tcp::TcpBuilder;
use rmp_serde::to_vec;
use std::{
    collections::HashMap,
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
    remote_addr: SocketAddr,
    cancel_token: CancellationToken,
    udp_task_handle: JoinHandle<()>,
    socket_task_handle: JoinHandle<()>,
}

pub struct Server {
    connections: Arc<Mutex<HashMap<String, ConnectionInfo>>>,
}

impl Server {
    pub fn new(log_level: LevelFilter) -> Server {
        env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", log_level.to_string()))
            .init();
        Server {
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn wait_for_stream_end(
        framed_stream: &mut Framed<TcpStream, LengthDelimitedCodec>,
        device_id: String,
        cancel_token: CancellationToken,
        sender: Sender<String>,
    ) {
        loop {
            select! {
                _ = cancel_token.cancelled() => {
                    break;
                }
                frame = framed_stream.next() => {
                    let _ = match frame {
                        Some(Ok(_)) => (),
                        _ => {
                            break;
                        }
                    };
                }
            }
        }

        sender.send(device_id.to_string()).ok();
    }

    fn add_connection(
        &self,
        remote_addr: &SocketAddr,
        output_device_cfg: crate::DeviceConfig,
        cancel_token: CancellationToken,
        udp_task_handle: JoinHandle<()>,
        socket_task_handle: JoinHandle<()>,
    ) {
        let mut locked_map = self.connections.lock().unwrap();
        locked_map.insert(
            output_device_cfg.device_id(),
            ConnectionInfo {
                remote_addr: *remote_addr,
                cancel_token,
                udp_task_handle,
                socket_task_handle,
            },
        );
    }

    async fn remove_connection(&self, device_id: &str) {
        let mut locked_map = self.connections.lock().unwrap();

        if let Some(info) = locked_map.remove(device_id) {
            info!("Disconnect from {}", info.remote_addr);
            info.cancel_token.cancel();
            join_all(
                vec![info.udp_task_handle, info.socket_task_handle]
                    .iter_mut()
                    .filter(|h| !h.is_finished()),
            )
            .await;
        }
    }

    async fn remove_all_connections(&self) {
        let mut handles = Vec::new();

        {
            let mut locked_map = self.connections.lock().unwrap();

            for (_, info) in locked_map.drain() {
                info.cancel_token.cancel();
                handles.push(info.udp_task_handle);
                handles.push(info.socket_task_handle);
            }
        }

        join_all(handles.into_iter().filter(|h| !h.is_finished())).await;
    }

    async fn handle_list_msg(
        &self,
        mut framed_stream: Framed<TcpStream, LengthDelimitedCodec>,
        remote_addr: &SocketAddr,
    ) -> anyhow::Result<()> {
        info!("Received device list message from {}", remote_addr);
        let output = AudioCaps::get_device_list_string().unwrap();
        let list_remote_response_message = NetworkMessage::ListResponse { output };

        framed_stream
            .send(to_vec(&list_remote_response_message).unwrap().into())
            .await?;
        Ok(())
    }

    async fn handle_send_msg(
        &self,
        host: String,
        device: Option<String>,
        stream_cfg: Option<String>,
        remote_udp_addr: &SocketAddr,
        local_addr: &SocketAddr,
        remote_addr: &SocketAddr,
        mut framed_stream: Framed<TcpStream, LengthDelimitedCodec>,
        sender: &Sender<String>,
    ) -> anyhow::Result<()> {
        let (input_device, input_device_cfg, buffer_frames) = AudioCaps::get_input_device(
            &host,
            &device,
            &stream_cfg.and_then(|s| StreamConfig::from_str(&s).ok()),
        )?;
        let device_id = input_device_cfg.device_id();

        self.remove_connection(&device_id).await;

        let local_udp_socket = UdpSocket::bind(SocketAddr::new(local_addr.ip(), 0))
            .await
            .context("Failed to bind a new UDP socket")?;
        let message = NetworkMessage::SendAudioResponse {
            actual_host: host,
            actual_device: input_device_cfg.device_name.to_owned(),
            actual_stream_cfg: input_device_cfg.stream_cfg.to_string(),
        };

        framed_stream.send(to_vec(&message)?.into()).await?;

        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();
        let input_device_cfg_clone = input_device_cfg.clone();
        let remote_udp_addr = remote_udp_addr.clone();
        let udp_task_handle = tokio::task::spawn_blocking(move || {
            // cpal::Stream is !Send so we must use a dedicated thread for the UdpServer
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                match UdpServer::send_audio(
                    local_udp_socket,
                    remote_udp_addr,
                    input_device,
                    input_device_cfg_clone,
                    buffer_frames,
                    cancel_token_clone,
                )
                .await
                {
                    Ok(_) => {}
                    Err(e) => error!("Error in UDP send audio task - {}", e),
                }
            });
        });

        let sender_clone = sender.clone();
        let device_id_clone = device_id.clone();
        let cancel_token_clone = cancel_token.clone();
        let socket_task_handle = tokio::spawn(async move {
            Self::wait_for_stream_end(
                &mut framed_stream,
                device_id_clone,
                cancel_token_clone,
                sender_clone,
            )
            .await;
        });

        {
            let mut locked_map = self.connections.lock().unwrap();

            locked_map.insert(
                device_id,
                ConnectionInfo {
                    remote_addr: *remote_addr,
                    cancel_token,
                    udp_task_handle,
                    socket_task_handle,
                },
            );
        }

        Ok(())
    }

    async fn handle_receive_msg(
        &self,
        host: String,
        device: Option<String>,
        config: Option<String>,
        local_addr: &SocketAddr,
        remote_addr: &SocketAddr,
        mut framed_stream: Framed<TcpStream, LengthDelimitedCodec>,
        sender: &Sender<String>,
    ) -> anyhow::Result<()> {
        let (output_device, output_device_cfg, buffer_frames) = AudioCaps::get_output_device(
            &host,
            &device,
            &config.and_then(|s| StreamConfig::from_str(&s).ok()),
        )?;

        self.remove_connection(&output_device_cfg.device_id()).await;

        let local_udp_socket = UdpSocket::bind(SocketAddr::new(local_addr.ip(), 0))
            .await
            .context("Failed to bind UDP socket")?;
        let local_udp_addr = local_udp_socket.local_addr()?;
        let message = NetworkMessage::ReceiveAudioResponse {
            actual_host: host,
            actual_device: output_device_cfg.device_name.to_owned(),
            actual_stream_cfg: output_device_cfg.stream_cfg.to_string(),
            udp_addr: local_udp_addr.to_string(),
        };

        framed_stream.send(to_vec(&message)?.into()).await?;

        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();
        let output_device_cfg_clone = output_device_cfg.clone();
        let udp_task_handle = tokio::task::spawn_blocking(move || {
            // cpal::Stream is !Send so we must use a dedicated thread for the UdpServer
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                match UdpServer::receive_audio(
                    local_udp_socket,
                    output_device,
                    output_device_cfg_clone,
                    buffer_frames,
                    cancel_token_clone,
                )
                .await
                {
                    Ok(_) => {}
                    Err(e) => error!("Error in UDP receive audio task - {}", e),
                }
            });
        });

        let output_device_cfg_clone = output_device_cfg.clone();
        let senders_clone = sender.clone();
        let cancel_token_clone = cancel_token.clone();
        let socket_task_handle = tokio::spawn(async move {
            Self::wait_for_stream_end(
                &mut framed_stream,
                output_device_cfg_clone.device_id(),
                cancel_token_clone,
                senders_clone,
            )
            .await;
        });

        self.add_connection(
            remote_addr,
            output_device_cfg,
            cancel_token,
            udp_task_handle,
            socket_task_handle,
        );

        Ok(())
    }

    async fn handle_connection(
        &self,
        stream: TcpStream,
        local_addr: &SocketAddr,
        remote_addr: &SocketAddr,
        sender: &Sender<String>,
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
            NetworkMessage::List => self.handle_list_msg(framed_stream, remote_addr).await,
            NetworkMessage::SendAudio {
                host,
                device,
                stream_cfg,
                udp_addr,
            } => {
                // Use the same IP address as the TCP connection for the UDP address
                let udp_addr: SocketAddr = udp_addr.parse()?;
                let remote_udp_addr = SocketAddr::new(remote_addr.ip(), udp_addr.port());

                self.handle_send_msg(
                    host,
                    device,
                    stream_cfg,
                    &remote_udp_addr,
                    local_addr,
                    remote_addr,
                    framed_stream,
                    sender,
                )
                .await
            }
            NetworkMessage::ReceiveAudio {
                host,
                device,
                stream_cfg,
            } => {
                self.handle_receive_msg(
                    host,
                    device,
                    stream_cfg,
                    local_addr,
                    remote_addr,
                    framed_stream,
                    sender,
                )
                .await
            }
            _ => Err(anyhow::anyhow!("Unexpected message from {}", remote_addr)),
        }
    }

    pub async fn listen(&self, local_addr: &SocketAddr) -> anyhow::Result<()> {
        let (sender, receiver) = flume::unbounded::<String>();
        let listener = TcpListener::bind(local_addr).await?;

        #[cfg(feature = "metrics")]
        {
            let metrics_builder = TcpBuilder::new().listen_address(METRICS_SERVER_SOCKADDR);

            metrics_builder.install().context(format!(
                "Unable to start metrics collection on {}",
                METRICS_SERVER_SOCKADDR
            ))?;
        }

        info!("Server listening on {}", local_addr);

        loop {
            select! {
                Ok((stream, remote_addr)) = listener.accept() => {
                    info!("Accepted connection from {}", remote_addr);

                    match self.handle_connection(stream, &local_addr, &remote_addr, &sender).await {
                        Ok(_) => {}
                        Err(e) => error!("Error handling connection - {}", e),
                    }
                }
                Ok(device_id) = receiver.recv_async() => {
                    self.remove_connection(&device_id).await;
                }
                _ = signal::ctrl_c() => {
                    break;
                }
            }
        }

        self.remove_all_connections().await;
        info!("Server stopped");

        Ok(())
    }
}
