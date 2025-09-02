use anyhow::Context;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    Device, FromSample, InputCallbackInfo, OutputCallbackInfo, Sample, SizedSample,
    SupportedStreamConfig,
};
use dasp_sample::ToSample;
use log::{debug, error, info, warn};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{net::UdpSocket, select, signal, sync::Notify};

pub struct UdpServer {}

impl UdpServer {
    pub fn new() -> UdpServer {
        UdpServer {}
    }

    pub async fn send_audio<T>(
        &self,
        sock_addr: &std::net::SocketAddr,
        device: &Device,
        supported_config: &SupportedStreamConfig,
    ) -> Result<(), anyhow::Error>
    where
        T: SizedSample + Sample + ToSample<f32>,
    {
        let config = supported_config.config();
        let channels = supported_config.channels() as usize;
        // TODO @john: Make configurable
        let audio_buffer: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(10000 * channels)));
        let audio_buffer_clone = audio_buffer.clone();
        // TODO @john: Pass this in
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("Failed to bind UDP socket and port")?;
        let mut sequence_number = 0u64;
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();
        let mut packet_buffer = Vec::with_capacity(crate::MTU);
        let stream = device.build_input_stream(
            &config,
            move |input: &[T], _: &InputCallbackInfo| {
                let mut audio_buffer = audio_buffer_clone.lock().unwrap();
                let mut audio_buffer_shrunk = false;

                // Prevent buffer overflow
                while audio_buffer.len() + input.len() > audio_buffer.capacity() {
                    audio_buffer.pop_front();
                    audio_buffer_shrunk = true;
                }

                if audio_buffer_shrunk {
                    debug!("Audio buffer shrunk");
                }

                for sample in input.iter() {
                    let sample = sample.to_sample::<f32>();
                    audio_buffer.push_back(sample);
                }

                notify_clone.notify_one();
            },
            move |err| {
                error!("Audio stream error - {}", err);
            },
            None,
        )?;

        stream.play()?;

        // It is expected that this loop will never exit, and will be called in the
        // context of a tokio task which can be aborted.
        loop {
            // Wait for the audio buffer to be ready
            notify.notified().await;

            {
                let mut audio_buffer = audio_buffer.lock().unwrap();

                debug!(
                    "Audio buffer size: {} ({:.2}%)",
                    audio_buffer.len(),
                    audio_buffer.len() as f32 / audio_buffer.capacity() as f32 * 100.0
                );

                while !audio_buffer.is_empty() {
                    packet_buffer.clear();
                    packet_buffer.extend_from_slice(&sequence_number.to_le_bytes());

                    while !audio_buffer.is_empty()
                        && packet_buffer.len() + 2 * size_of::<f32>() <= crate::MTU
                    {
                        let sample = audio_buffer.pop_front().unwrap();
                        let sample_slice = &sample.to_le_bytes();

                        packet_buffer.extend_from_slice(sample_slice);

                        // Duplicate the sample for mono input channels
                        if channels == 1 {
                            packet_buffer.extend_from_slice(sample_slice);
                        }
                    }

                    sequence_number += 1;

                    match socket.send_to(&packet_buffer, sock_addr).await {
                        Ok(len) => {
                            if len != packet_buffer.len() {
                                warn!("Partial packet sent");
                            }
                        }
                        Err(e) => error!("Failed to send audio packet - {}", e),
                    }
                }
            }
        }
    }

    pub async fn receive_audio<T>(
        &self,
        sock_addr: &std::net::SocketAddr,
        device: &Device,
        supported_config: &SupportedStreamConfig,
    ) -> anyhow::Result<()>
    where
        T: SizedSample + FromSample<f32>,
    {
        let config = supported_config.config();
        let channels = supported_config.channels() as usize;
        // TODO @john: Make audio buffer size configurable
        let audio_buffer: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(120000 * channels)));
        let audio_buffer_clone = audio_buffer.clone();
        let mut packet_buffer = vec![0u8; crate::MTU];
        let socket = UdpSocket::bind(sock_addr)
            .await
            .context("Failed to bind UDP socket")?;
        let stream = device.build_output_stream(
            &config,
            move |output: &mut [T], _: &OutputCallbackInfo| {
                let mut audio_buffer = audio_buffer_clone.lock().unwrap();

                if audio_buffer.len() > 0 {
                    debug!(
                        "Audio buffer length {} ({}%)",
                        audio_buffer.len(),
                        audio_buffer.len() as f32 / audio_buffer.capacity() as f32 * 100.0
                    );
                }

                for sample in output.iter_mut() {
                    if let Some(value) = audio_buffer.pop_front() {
                        *sample = T::from_sample(value);
                    } else {
                        *sample = Sample::EQUILIBRIUM;
                    }
                }
            },
            move |err| {
                error!("Audio stream error - {}", err);
            },
            None,
        )?;

        let sequence_number: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
        let mut packet_length = 0;

        stream.play()?;

        loop {
            select! {
                socket_result = socket.recv_from(&mut packet_buffer) => {
                    match socket_result {
                        Ok((len, _addr)) => {
                            packet_length = len;
                        }
                        Err(e) => error!("Failed to receive audio packet - {}", e)
                    }
                }
                _ = signal::ctrl_c() => {
                    info!("Stopping service");
                    break;
                }
            }

            {
                let mut sequence_number = sequence_number.lock().unwrap();
                let next_sequence_number;

                if packet_length >= size_of::<u64>() {
                    next_sequence_number =
                        u64::from_le_bytes(packet_buffer[..size_of::<u64>()].try_into().unwrap());
                } else {
                    // Packet too small
                    debug!("Audio packet too small");
                    continue;
                }

                if next_sequence_number <= *sequence_number {
                    debug!("Audio data length not divisible by sample size");
                    continue;
                }

                *sequence_number = next_sequence_number;
                debug!(
                    "Received packet {}, packet size: {}",
                    next_sequence_number, packet_length,
                );

                // Extract audio data
                let audio_data = &packet_buffer[size_of::<u64>()..packet_length];

                // Convert bytes to samples
                if audio_data.len() % size_of::<f32>() != 0 {
                    // Audio data length not divisible by sample size
                    debug!("Audio data length not divisible by 4 bytes");
                    continue;
                }

                {
                    let mut audio_buffer = audio_buffer.lock().unwrap();
                    let audio_data_chunks = audio_data.chunks_exact(size_of::<f32>());
                    let mut audio_buffer_shrunk = false;

                    // Prevent buffer overflow
                    while audio_buffer.len() + audio_data_chunks.len() > audio_buffer.capacity() {
                        audio_buffer.pop_front();
                        audio_buffer_shrunk = true;
                    }

                    if audio_buffer_shrunk {
                        debug!("Audio buffer shrunk - audio lost");
                    }

                    for chunk in audio_data_chunks {
                        let sample = f32::from_le_bytes(chunk.try_into().unwrap());
                        audio_buffer.push_back(sample);
                    }
                }
            }
        }

        Ok(())
    }
}
