pub use crate::{audio_caps::AudioCaps, messages::NetworkMessage, udp_server::UdpServer};
use env_logger::Env;
use futures::{SinkExt, StreamExt};
use log::{error, info, LevelFilter};
use rmp_serde::to_vec;
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    select, signal,
    sync::Mutex,
    task,
    task::JoinHandle,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub struct Server {
    task_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Server {
    pub fn new(log_level: LevelFilter) -> Server {
        env_logger::Builder::from_env(Env::default().filter_or("RUST_LOG", log_level.to_string()))
            .init();
        Server {
            task_handle: Arc::new(Mutex::new(None)),
        }
    }

    async fn handle_connection(&self, socket: TcpStream) -> anyhow::Result<()> {
        let mut framed = Framed::new(socket, LengthDelimitedCodec::new());

        // Process all the messages from the stream
        while let Some(result) = framed.next().await {
            let frame = result?;

            // Deserialize the received bytes
            let message = rmp_serde::from_slice::<NetworkMessage>(&frame)?;

            // Match on the deserialized enum
            match message {
                NetworkMessage::List => {
                    info!("Handling list remote message");
                    let output = AudioCaps::list_to_string().unwrap();
                    let list_remote_response_message = NetworkMessage::ListResponse { output };

                    framed
                        .send(to_vec(&list_remote_response_message).unwrap().into())
                        .await?;
                }
                NetworkMessage::SendAudio {
                    host,
                    device,
                    stream_config,
                } => {
                    info!("Handling send audio to remote message");

                    // TODO @john: Look up the host/device/stream config
                    // TODO @john: Respond with the actual host/device/stream information or error message

                    {
                        let mut handle_lock = self.task_handle.lock().await;

                        if let Some(ref mut join_handle) = *handle_lock {
                            join_handle.abort();
                        }

                        *handle_lock = Some(task::spawn(async {
                            //UdpServer::new().send_audio(sock_addr, device, supported_config);
                        }));
                    }
                }
                NetworkMessage::ReceiveAudio {
                    host,
                    device,
                    stream_config,
                } => {
                    info!("Handling receive audio to remote message");

                    // TODO @john: Look up the host/device/stream config
                    // TODO @john: Respond with the actual host/device/stream information or error message

                    {
                        let mut handle_lock = self.task_handle.lock().await;

                        if let Some(ref mut join_handle) = *handle_lock {
                            join_handle.abort();
                        }

                        let join_handle = task::spawn(async {
                            UdpServer::new()
                                .send_audio(sock_addr, device, supported_config)
                                .await;
                        });

                        *handle_lock = Some(join_handle);
                    }
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
            // TODO @john: Create a signal that can be used to stop the server

            select! {
                Ok((socket, addr)) = listener.accept() => {
                    info!("Accepted connection from {}", addr);
                    match self.handle_connection(socket).await {
                        Ok(_) => {}
                        Err(e) => error!("Error handling connection: {}", e),
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
