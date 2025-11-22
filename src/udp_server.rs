use crate::DeviceConfig;
use cpal::{
    traits::{DeviceTrait, StreamTrait},
    Device, FromSample, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat, SizedSample,
};
use dasp_sample::ToSample;
use metrics::{counter, describe_counter, describe_gauge, gauge, Unit};
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
                    cancel_token,
                )
                .await
            }
            _ => Err(anyhow::anyhow!(
                "Only f32 and i16 sample formats are supported on input device"
            )),
        }
    }

    fn create_send_metrics(
        device_cfg: &DeviceConfig,
    ) -> (
        metrics::Gauge,
        metrics::Gauge,
        metrics::Counter,
        metrics::Counter,
        metrics::Counter,
        metrics::Counter,
    ) {
        let buffer_length_gauge_name = format!(
            "audio_buffer_length_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let buffer_percent_full_gauge_name = format!(
            "audio_buffer_percent_full_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let buffer_truncated_counter_name = format!(
            "audio_buffer_truncated_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let partial_send_counter_name = format!(
            "audio_buffer_partial_send_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let stream_error_counter_name = format!(
            "audio_buffer_stream_error_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let send_error_counter_name = format!(
            "audio_buffer_send_error_{}",
            device_cfg.to_string().replace('|', "_")
        );

        describe_gauge!(
            buffer_length_gauge_name.to_owned(),
            Unit::Bytes,
            "Length of the audio buffer in samples"
        );
        describe_gauge!(
            buffer_percent_full_gauge_name.to_owned(),
            Unit::Percent,
            "Percentage of the audio buffer that is full"
        );
        describe_counter!(
            buffer_truncated_counter_name.to_owned(),
            Unit::Count,
            "Audio buffer was truncated to prevent overflow"
        );
        describe_counter!(
            partial_send_counter_name.to_owned(),
            Unit::Count,
            "Samples dropped due to partial UDP buffer sends"
        );
        describe_counter!(
            stream_error_counter_name.to_owned(),
            Unit::Count,
            "Audio stream errors"
        );
        describe_counter!(
            send_error_counter_name.to_owned(),
            Unit::Count,
            "Audio buffer UDP send errors"
        );
        let audio_buffer_length_gauge = gauge!(buffer_length_gauge_name);
        let audio_buffer_percent_full_gauge = gauge!(buffer_percent_full_gauge_name);
        let audio_buffer_truncated_counter = counter!(buffer_truncated_counter_name);
        let audio_partial_send_counter = counter!(partial_send_counter_name);
        let audio_stream_error_counter = counter!(stream_error_counter_name);
        let audio_send_error_counter = counter!(send_error_counter_name);
        (
            audio_buffer_length_gauge,
            audio_buffer_percent_full_gauge,
            audio_buffer_truncated_counter,
            audio_partial_send_counter,
            audio_stream_error_counter,
            audio_send_error_counter,
        )
    }

    async fn inner_send_audio<T>(
        socket: &UdpSocket,
        sock_addr: &SocketAddr,
        device: &Device,
        device_cfg: &DeviceConfig,
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
        let (
            audio_buffer_length_gauge,
            audio_buffer_percent_full_gauge,
            audio_buffer_truncated_counter,
            audio_partial_send_counter,
            audio_stream_error_counter,
            audio_send_error_counter,
        ) = UdpServer::create_send_metrics(device_cfg);

        let stream = device.build_input_stream(
            &cpal::StreamConfig {
                channels: device_cfg.stream_cfg.channels,
                sample_rate: cpal::SampleRate(device_cfg.stream_cfg.sample_rate),
                buffer_size: cpal::BufferSize::Default,
            },
            move |input: &[T], _: &InputCallbackInfo| {
                let mut audio_buffer = audio_buffer_clone.lock().unwrap();

                // Prevent buffer overflow
                if audio_buffer.len() + input.len() > audio_buffer.capacity() {
                    audio_buffer_truncated_counter.increment(1);

                    while audio_buffer.len() + input.len() > audio_buffer.capacity() {
                        audio_buffer.pop_front();
                    }
                }

                for sample in input.iter() {
                    let sample = sample.to_sample::<f32>();
                    audio_buffer.push_back(sample);
                }

                notify_clone.notify_one();
            },
            move |err| {
                audio_stream_error_counter.increment(1);
                log::debug!("Audio stream error - {}", err);
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

                audio_buffer_length_gauge.set(audio_buffer.len() as f64);
                audio_buffer_percent_full_gauge.set(
                    100.0 * (audio_buffer.len() as f32 / audio_buffer.capacity() as f32) as f64,
                );

                // Convert audio samples to bytes and send in UDP packets
                while !audio_buffer.is_empty() {
                    packet_buffer.clear();
                    packet_buffer.extend_from_slice(&sequence_number.to_le_bytes());

                    if device_cfg.stream_cfg.channels == 1 {
                        const NUM_CHANNELS: usize = 2;

                        while !audio_buffer.is_empty()
                            && packet_buffer.len() + NUM_CHANNELS * size_of::<f32>() <= crate::MTU
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
                                audio_partial_send_counter.increment(1);
                            }
                        }
                        Err(e) => {
                            audio_send_error_counter.increment(1);
                            log::debug!("Failed to send audio packet to - {}", e);
                        }
                    }
                }
            }
        }

        log::info!("Stopped sending audio to udp://{}", sock_addr);
        Ok(())
    }

    fn create_receive_metrics(
        device_cfg: &DeviceConfig,
    ) -> (
        metrics::Gauge,
        metrics::Gauge,
        metrics::Counter,
        metrics::Counter,
        metrics::Counter,
        metrics::Gauge,
        metrics::Counter,
        metrics::Counter,
        metrics::Counter,
    ) {
        let buffer_length_gauge_name = format!(
            "audio_buffer_length_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let buffer_percent_full_gauge_name = format!(
            "audio_buffer_percent_full_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let buffer_truncated_counter_name = format!(
            "audio_buffer_truncated_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let stream_error_counter_name = format!(
            "audio_stream_error_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let packet_receive_error_counter_name = format!(
            "audio_packet_receive_error_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let packet_length_gauge_name = format!(
            "audio_packet_length_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let packet_too_small_counter_name = format!(
            "audio_packet_too_small_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let packet_out_of_order_counter_name = format!(
            "audio_packet_out_of_order_{}",
            device_cfg.to_string().replace('|', "_")
        );
        let packet_bad_size_counter_name = format!(
            "audio_packet_bad_size_{}",
            device_cfg.to_string().replace('|', "_")
        );

        describe_gauge!(
            buffer_length_gauge_name.to_owned(),
            Unit::Bytes,
            "Length of the audio buffer in samples"
        );
        describe_gauge!(
            buffer_percent_full_gauge_name.to_owned(),
            Unit::Percent,
            "Percentage of the audio buffer that is full"
        );
        describe_counter!(
            buffer_truncated_counter_name.to_owned(),
            Unit::Count,
            "Audio buffer was truncated to prevent overflow"
        );
        describe_counter!(
            stream_error_counter_name.to_owned(),
            Unit::Count,
            "Audio stream errors"
        );
        describe_counter!(
            packet_receive_error_counter_name.to_owned(),
            Unit::Count,
            "Received audio packet UDP errors"
        );
        describe_gauge!(
            packet_length_gauge_name.to_owned(),
            Unit::Percent,
            "Received audio packet length"
        );
        describe_counter!(
            packet_too_small_counter_name.to_owned(),
            Unit::Count,
            "Received audio packet too small"
        );
        describe_counter!(
            packet_out_of_order_counter_name.to_owned(),
            Unit::Count,
            "Received audio packet out of order"
        );
        describe_counter!(
            packet_bad_size_counter_name.to_owned(),
            Unit::Count,
            "Received audio packet bad size"
        );
        let audio_buffer_length_gauge = gauge!(buffer_length_gauge_name);
        let audio_buffer_percent_full_gauge = gauge!(buffer_percent_full_gauge_name);
        let audio_buffer_truncated_counter = counter!(buffer_truncated_counter_name);
        let audio_stream_error_counter = counter!(stream_error_counter_name);
        let audio_packet_receive_error_counter = counter!(packet_receive_error_counter_name);
        let audio_packet_length_gauge = gauge!(packet_length_gauge_name);
        let audio_packet_too_small_counter = counter!(packet_too_small_counter_name);
        let audio_packet_out_of_order_counter = counter!(packet_out_of_order_counter_name);
        let audio_packet_bad_size_counter = counter!(packet_bad_size_counter_name);
        (
            audio_buffer_length_gauge,
            audio_buffer_percent_full_gauge,
            audio_buffer_truncated_counter,
            audio_stream_error_counter,
            audio_packet_receive_error_counter,
            audio_packet_length_gauge,
            audio_packet_too_small_counter,
            audio_packet_out_of_order_counter,
            audio_packet_bad_size_counter,
        )
    }

    pub async fn receive_audio(
        socket: UdpSocket,
        device: Device,
        device_cfg: DeviceConfig,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<()> {
        if device_cfg.stream_cfg.channels != 2 {
            return Err(anyhow::anyhow!(
                "Only 2 channels are supported on output device"
            ));
        }

        match device_cfg.stream_cfg.sample_format {
            SampleFormat::F32 => {
                Self::inner_receive_audio::<f32>(&socket, &device, &device_cfg, cancel_token).await
            }
            SampleFormat::U16 => {
                Self::inner_receive_audio::<i32>(&socket, &device, &device_cfg, cancel_token).await
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
        cancel_token: CancellationToken,
    ) -> anyhow::Result<()>
    where
        T: SizedSample + FromSample<f32>,
    {
        let mut packet_buffer = vec![0u8; crate::MTU];
        // TODO(john): Calculate buffer size based on input parameters
        let audio_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(1024 * 16)));
        let audio_buffer_clone = audio_buffer.clone();
        let (
            audio_buffer_length_gauge,
            audio_buffer_percent_full_gauge,
            audio_buffer_truncated_counter,
            audio_stream_error_counter,
            audio_packet_receive_error_counter,
            audio_packet_length_gauge,
            audio_packet_too_small_counter,
            audio_packet_out_of_order_counter,
            audio_packet_bad_size_counter,
        ) = UdpServer::create_receive_metrics(device_cfg);
        let stream = device.build_output_stream(
            &cpal::StreamConfig {
                channels: device_cfg.stream_cfg.channels,
                sample_rate: cpal::SampleRate(device_cfg.stream_cfg.sample_rate),
                buffer_size: cpal::BufferSize::Default,
            },
            move |output: &mut [T], _: &OutputCallbackInfo| {
                let mut audio_buffer = audio_buffer_clone.lock().unwrap();

                if audio_buffer.len() > 0 {
                    audio_buffer_length_gauge.set(audio_buffer.len() as f64);
                    audio_buffer_percent_full_gauge.set(
                        100.0 * (audio_buffer.len() as f32 / audio_buffer.capacity() as f32) as f64,
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
                audio_stream_error_counter.increment(1);
                log::debug!("Audio stream error - {}", err);
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
                            audio_packet_receive_error_counter.increment(1);
                            log::debug!("Failed to receive audio packet - {}", e)
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
                    audio_packet_too_small_counter.increment(1);
                    continue;
                }

                if next_sequence_number <= *sequence_number {
                    audio_packet_out_of_order_counter.increment(1);
                    continue;
                }

                *sequence_number = next_sequence_number;
                audio_packet_length_gauge.set(packet_length as f64);

                // Extract audio data
                let audio_data = &packet_buffer[size_of::<u64>()..packet_length];

                // Convert bytes to samples
                if audio_data.len() % size_of::<f32>() != 0 {
                    audio_packet_bad_size_counter.increment(1);
                    continue;
                }

                {
                    let mut audio_buffer = audio_buffer.lock().unwrap();
                    let audio_data_chunks = audio_data.chunks_exact(size_of::<f32>());

                    // Prevent buffer overflow by removing old samples
                    if audio_buffer.len() + audio_data_chunks.len() > audio_buffer.capacity() {
                        audio_buffer_truncated_counter.increment(1);

                        while audio_buffer.len() + audio_data_chunks.len() > audio_buffer.capacity()
                        {
                            audio_buffer.pop_front();
                        }
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
