use crate::DeviceConfig;
use anyhow::bail;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    Device, FromSample, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat, SizedSample,
};
use dasp_sample::ToSample;
use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{net::UdpSocket, select, sync::Notify};
use tokio_util::sync::CancellationToken;

pub struct UdpServer {}

impl UdpServer {
    pub async fn send_audio(
        socket: UdpSocket,
        sock_addr: SocketAddr,
        device: Device,
        device_cfg: DeviceConfig,
        buffer_frames: u32,
        cancel_token: CancellationToken,
    ) -> Result<(), anyhow::Error> {
        match device_cfg.stream_cfg.sample_format {
            SampleFormat::F32 => {
                Self::gen_send_audio::<f32>(
                    &socket,
                    &sock_addr,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            SampleFormat::I16 => {
                Self::gen_send_audio::<i16>(
                    &socket,
                    &sock_addr,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            SampleFormat::U16 => {
                Self::gen_send_audio::<u16>(
                    &socket,
                    &sock_addr,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            _ => bail!("Unsupported sample format on input device"),
        }
    }

    async fn gen_send_audio<T>(
        socket: &UdpSocket,
        sock_addr: &SocketAddr,
        device: &Device,
        device_cfg: &DeviceConfig,
        buffer_frames: u32,
        cancel_token: CancellationToken,
    ) -> Result<(), anyhow::Error>
    where
        T: SizedSample + Sample + ToSample<f32>,
    {
        if device_cfg.stream_cfg.channels != 1 && device_cfg.stream_cfg.channels != 2 {
            return Err(anyhow::anyhow!(
                "Only 1 or 2 channels are supported on input device"
            ));
        }

        let mut sequence_number = 0u64;
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();
        let mut packet_buffer = Vec::with_capacity(crate::MTU);
        let audio_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(1024 * 16)));
        let audio_buffer_clone = audio_buffer.clone();
        let stream = device.build_input_stream(
            &cpal::StreamConfig {
                channels: device_cfg.stream_cfg.channels,
                sample_rate: cpal::SampleRate(device_cfg.stream_cfg.sample_rate),
                buffer_size: cpal::BufferSize::Fixed(buffer_frames),
                // buffer_size: cpal::BufferSize::Default,
            },
            move |input: &[T], _: &InputCallbackInfo| {
                let mut audio_buffer = audio_buffer_clone.lock().unwrap();
                let mut audio_buffer_shrunk = false;

                // Prevent buffer overflow
                while audio_buffer.len() + input.len() > audio_buffer.capacity() {
                    audio_buffer.pop_front();
                    audio_buffer_shrunk = true;
                }

                if audio_buffer_shrunk {
                    log::trace!("Audio buffer shrunk");
                }

                for sample in input.iter() {
                    let sample = sample.to_sample::<f32>();
                    audio_buffer.push_back(sample);
                }

                notify_clone.notify_one();
            },
            move |err| {
                log::error!("Audio stream error - {}", err);
            },
            None,
        )?;

        log::info!(
            "Sending local input audio device {} to udp://{}",
            device_cfg,
            sock_addr,
        );

        stream.play()?;

        loop {
            select! {
                _ = cancel_token.cancelled() => {
                    break;
                }
                _ = notify.notified() => {}
            }

            {
                let mut audio_buffer = audio_buffer.lock().unwrap();

                log::trace!(
                    "Audio buffer length: {} ({:.2}%)",
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
                        if device_cfg.stream_cfg.channels == 1 {
                            packet_buffer.extend_from_slice(sample_slice);
                        } else {
                            // TODO(john): Add a flag to copy left or right channel over the other
                            // to support devices that have stereo input but only one channel is used
                            // We will use the same sample_slice again and pop the next sample from
                            // the audio_buffer
                        }
                    }

                    sequence_number += 1;

                    match socket.send_to(&packet_buffer, sock_addr).await {
                        Ok(len) => {
                            if len != packet_buffer.len() {
                                log::warn!("Partial packet sent");
                            }
                        }
                        Err(e) => {
                            // TODO(john): Decide how to handle errors in the UDP sending task
                            log::error!("Failed to send audio packet to {} - {}", sock_addr, e);
                        }
                    }
                }
            }
        }

        log::info!("Stopped sending audio to udp://{}", sock_addr);
        Ok(())
    }

    pub async fn receive_audio(
        socket: UdpSocket,
        device: Device,
        device_cfg: DeviceConfig,
        buffer_frames: u32,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<()> {
        match device_cfg.stream_cfg.sample_format {
            SampleFormat::F32 => {
                Self::gen_receive_audio::<f32>(
                    &socket,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            SampleFormat::I16 => {
                Self::gen_receive_audio::<i16>(
                    &socket,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            SampleFormat::U16 => {
                Self::gen_receive_audio::<u16>(
                    &socket,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            _ => bail!("Unsupported sample format on output device"),
        }
    }

    pub async fn gen_receive_audio<T>(
        udp_socket: &UdpSocket,
        device: &Device,
        device_cfg: &DeviceConfig,
        buffer_frames: u32,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<()>
    where
        T: SizedSample + FromSample<f32>,
    {
        if device_cfg.stream_cfg.channels != 2 {
            return Err(anyhow::anyhow!(
                "Only 2 channels are supported on output device"
            ));
        }

        let mut packet_buffer = vec![0u8; crate::MTU];
        let audio_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(1024 * 16)));
        let audio_buffer_clone = audio_buffer.clone();

        log::debug!(
            "Starting receive audio task {} (frames {})",
            device_cfg,
            buffer_frames
        );

        let stream = device.build_output_stream(
            &cpal::StreamConfig {
                channels: device_cfg.stream_cfg.channels,
                sample_rate: cpal::SampleRate(device_cfg.stream_cfg.sample_rate),
                buffer_size: cpal::BufferSize::Fixed(buffer_frames),
            },
            move |output: &mut [T], _: &OutputCallbackInfo| {
                let mut audio_buffer = audio_buffer_clone.lock().unwrap();

                if audio_buffer.len() > 0 {
                    log::trace!(
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
                log::error!("Audio stream error - {}", err);
            },
            None,
        )?;

        log::info!(
            "Receiving udp://{} to local audio output device {}",
            udp_socket.local_addr()?,
            device_cfg,
        );

        stream.play()?;

        let sequence_number: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
        let mut packet_length = 0;

        loop {
            select! {
                socket_result = udp_socket.recv_from(&mut packet_buffer) => {
                    match socket_result {
                        Ok((len, _addr)) => {
                            packet_length = len;
                        }
                        Err(e) => {
                            // TODO(john): Decide how to handle errors in the UDP receiving task
                            log::error!("Failed to receive audio packet - {}", e)
                        }
                    }
                }
                _ = cancel_token.cancelled() => {
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
                    log::trace!("Audio packet too small");
                    continue;
                }

                if next_sequence_number <= *sequence_number {
                    log::trace!("Audio data length not divisible by sample size");
                    continue;
                }

                *sequence_number = next_sequence_number;
                log::trace!(
                    "Received packet {}, packet size: {}",
                    next_sequence_number,
                    packet_length,
                );

                // Extract audio data
                let audio_data = &packet_buffer[size_of::<u64>()..packet_length];

                // Convert bytes to samples
                if audio_data.len() % size_of::<f32>() != 0 {
                    // Audio data length not divisible by sample size
                    log::trace!("Audio data length not divisible by 4 bytes");
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
                        log::trace!("Audio buffer shrunk - audio lost");
                    }

                    for chunk in audio_data_chunks {
                        let sample = f32::from_le_bytes(chunk.try_into().unwrap());
                        audio_buffer.push_back(sample);
                    }
                }
            }
        }

        log::info!(
            "Stopped receiving audio from udp://{}",
            udp_socket.local_addr()?
        );

        Ok(())
    }
}
