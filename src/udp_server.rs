use crate::DeviceConfig;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    Device, FromSample, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat, SizedSample,
};
use dasp_sample::ToSample;
#[cfg(feature = "metrics")]
use metrics::{describe_gauge, gauge};
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
        if device_cfg.stream_cfg.channels != 1 && device_cfg.stream_cfg.channels != 2 {
            return Err(anyhow::anyhow!(
                "Only 1 or 2 channels are supported on input device"
            ));
        }

        match device_cfg.stream_cfg.sample_format {
            SampleFormat::F32 => {
                Self::inner_send_audio::<f32>(
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
                Self::inner_send_audio::<i32>(
                    &socket,
                    &sock_addr,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            _ => Err(anyhow::anyhow!(
                "Only f32 and i16 sample formats are supported on input device"
            )),
        }
    }

    async fn inner_send_audio<T>(
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
        let mut sequence_number = 0u64;
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();
        let mut packet_buffer = Vec::with_capacity(crate::MTU);
        // TODO(john): Calculate buffer size based on input parameters
        let audio_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(1024 * 16)));
        let audio_buffer_clone = audio_buffer.clone();

        #[cfg(feature = "metrics")]
        {
            let buffer_used_gauge_name = format!(
                "audio_buffer_used_{}",
                device_cfg.to_string().replace('|', "_")
            );
            let buffer_percent_full_gauge_name = format!(
                "audio_buffer_percent_full_{}",
                device_cfg.to_string().replace('|', "_")
            );

            describe_gauge!(
                buffer_used_gauge_name.to_owned(),
                "Length of the audio buffer in samples"
            );
            describe_gauge!(
                buffer_percent_full_gauge_name.to_owned(),
                "Percentage of the audio buffer that is full"
            );
            let audio_buffer_used_gauge = gauge!(buffer_used_gauge_name);
            let audio_buffer_percent_full_gauge = gauge!(buffer_percent_full_gauge_name);
        }

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

                #[cfg(feature = "metrics")]
                {
                    audio_buffer_used_gauge.set(audio_buffer.len() as f64);
                    audio_buffer_percent_full_gauge.set(
                        (audio_buffer.len() as f32 / audio_buffer.capacity() as f32 * 100.0) as f64,
                    );
                }

                // Convert audio samples to bytes and send in UDP packets
                while !audio_buffer.is_empty() {
                    packet_buffer.clear();
                    packet_buffer.extend_from_slice(&sequence_number.to_le_bytes());

                    if device_cfg.stream_cfg.channels == 1 {
                        while !audio_buffer.is_empty()
                            && packet_buffer.len() + 2 * size_of::<f32>() <= crate::MTU
                        {
                            let sample_slice = &audio_buffer.pop_front().unwrap().to_le_bytes();

                            // Double mono samples for stereo output
                            packet_buffer.extend_from_slice(sample_slice);
                            packet_buffer.extend_from_slice(sample_slice);
                        }
                    } else {
                        while !audio_buffer.is_empty()
                            && packet_buffer.len() + 2 * size_of::<f32>() <= crate::MTU
                        {
                            let sample_slice = &audio_buffer.pop_front().unwrap().to_le_bytes();

                            packet_buffer.extend_from_slice(sample_slice);
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
        if device_cfg.stream_cfg.channels != 2 {
            return Err(anyhow::anyhow!(
                "Only 2 channels are supported on output device"
            ));
        }

        match device_cfg.stream_cfg.sample_format {
            SampleFormat::F32 => {
                Self::inner_receive_audio::<f32>(
                    &socket,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            SampleFormat::U16 => {
                Self::inner_receive_audio::<i32>(
                    &socket,
                    &device,
                    &device_cfg,
                    buffer_frames,
                    cancel_token,
                )
                .await
            }
            _ => Err(anyhow::anyhow!(
                "Only f32 and i32 sample formats are supported on output device"
            )),
        }
    }

    pub async fn inner_receive_audio<T>(
        udp_socket: &UdpSocket,
        device: &Device,
        device_cfg: &DeviceConfig,
        buffer_frames: u32,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<()>
    where
        T: SizedSample + FromSample<f32>,
    {
        let mut packet_buffer = vec![0u8; crate::MTU];
        // TODO(john): Calculate buffer size based on input parameters
        let audio_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(1024 * 16)));
        let audio_buffer_clone = audio_buffer.clone();
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
