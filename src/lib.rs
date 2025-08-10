//! This program allows you to forward audio from one device to another over the network.
//! Audio can be read from the network and written to an audio device, or it can be read from
//! an audio device and written out to the network.
//!
//! Audio transmission is done over UDP for performance and a separate TCP/IP connection is used
//! as a control channel and to configure the audio stream.
//!
//! You must run two instances of the program, one in `send` mode and one in `receive` mode. The instance
//! that is started first will be the server (`--server` flag), and the second instance will be a client.
//! The server instance will listen for incoming connections on a specified port, and the client instance
//! will connect to the server instance on the specified port.  The server is persistent and will continue
//! to listen for incoming connections until it is manually stopped.  It is recommended to run the server as
//! a `systemd` service on Linux.
//!
//! Audio that is read from a 1-channel audio device will be written to the network as a 2-channel audio.
//!
//! Audio that is read from a 1-channel audio device will be written to the network
//! as a 2-channel audio.
//!
//! Network packets are sent as 32-bit floating point values in the range of -1.0 to 1.0.
//!
//! Configurations are specified in the format `<channels>x<khz>x<format>`
//!
//! - `channels` - The number of channels in the audio stream. For example, 1 or 2.
//! - `khz` - The sample rate of the audio stream in kilohertz. For example, 44.1 or 48.
//! - `format` - The sample format of the audio stream. For example, f32, i16, u8.
//!
//! The first letter of the `format` is the type of the data (`i`, `u` or `f`), and the second letter
//! is the number of bits per sample.  For example, f32 is a 32-bit floating point number, i16 is a 16-bit
//! signed integer, u8 is an 8-bit unsigned integer.
//!
//! When listing the available audio devices, the format is `<channels>x<min-khz>-<max-khz>x<format>`.
//! You can specify any sample rate in the given range when specifying the audio device configuration
//! for the `send` or `receive` commands.
//!
use anyhow::{anyhow, Context, Result};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, FromSample, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat, SampleRate,
    SizedSample, SupportedStreamConfig, SupportedStreamConfigRange,
};
use dasp_sample::ToSample;
use log::{debug, error, info, warn};
use std::net::SocketAddr;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{net::UdpSocket, select, signal, sync::Notify};

// TODO @john: Make this configurable
const MTU: usize = 65536;

#[derive(Debug, Clone)]
pub struct StreamConfig {
    channels: u16,
    sample_rate: u32,
    sample_format: SampleFormat,
}

impl StreamConfig {
    pub fn parse(s: &str) -> Result<StreamConfig, anyhow::Error> {
        let mut parts = s.split('x');
        let channels = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing channels"))?
            .parse()?;
        let sample_rate: f32 = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing sample rate"))?
            .parse()?;
        let sample_format_str = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing sample format"))?;
        let sample_format = match sample_format_str {
            "i8" => SampleFormat::I8,
            "i16" => SampleFormat::I16,
            "i24" => SampleFormat::I24,
            "i32" => SampleFormat::I32,
            "i64" => SampleFormat::I64,
            "u8" => SampleFormat::U8,
            "u16" => SampleFormat::U16,
            "u32" => SampleFormat::U32,
            "u64" => SampleFormat::U64,
            "f32" => SampleFormat::F32,
            _ => anyhow::bail!("Unsupported sample format"),
        };
        Ok(StreamConfig {
            channels,
            sample_rate: (sample_rate * 1000.0) as u32,
            sample_format,
        })
    }
}

pub struct AudioForwarder {}

impl AudioForwarder {
    pub fn new() -> AudioForwarder {
        AudioForwarder {}
    }

    pub async fn receive(
        &self,
        sock_addr: &SocketAddr,
        host_name: &str,
        device_name: &Option<String>,
        stream_config: &Option<StreamConfig>,
    ) -> Result<(), anyhow::Error> {
        let (device, config) =
            Self::get_output_device_config(host_name, device_name, stream_config)?;

        info!(
            "Receiving audio from {} to {} -> {} -> {} channel{} at {} Hz",
            sock_addr,
            host_name,
            device.name().unwrap_or("Unknown".to_string()),
            config.channels(),
            if config.channels() == 1 { "" } else { "s" },
            config.sample_rate().0,
        );

        match config.sample_format() {
            SampleFormat::F32 => {
                self.receive_audio::<f32>(sock_addr, &device, &config)
                    .await?
            }
            SampleFormat::I16 => {
                self.receive_audio::<i16>(sock_addr, &device, &config)
                    .await?
            }
            SampleFormat::U16 => {
                self.receive_audio::<u16>(sock_addr, &device, &config)
                    .await?
            }
            _ => panic!("Unsupported sample format on output device"),
        }

        Ok(())
    }

    pub async fn send(
        &self,
        sock_addr: &SocketAddr,
        host_name: &str,
        device_name: &Option<String>,
        stream_config: &Option<StreamConfig>,
    ) -> Result<(), anyhow::Error> {
        let (device, config) =
            Self::get_input_device_config(host_name, device_name, stream_config)?;

        info!(
            "Sending audio from {} -> {} -> {} channel{} at {} Hz to {}",
            host_name,
            device.name().unwrap_or("Unknown".to_string()),
            config.channels(),
            if config.channels() == 1 { "" } else { "s" },
            config.sample_rate().0,
            sock_addr,
        );

        match config.sample_format() {
            SampleFormat::F32 => self.send_audio::<f32>(sock_addr, &device, &config).await?,
            SampleFormat::I16 => self.send_audio::<i16>(sock_addr, &device, &config).await?,
            SampleFormat::U16 => self.send_audio::<u16>(sock_addr, &device, &config).await?,
            _ => panic!("Unsupported sample format on input device"),
        }
        Ok(())
    }

    fn get_output_device_config(
        host_name: &str,
        device_name: &Option<String>,
        sample_config: &Option<StreamConfig>,
    ) -> Result<(Device, SupportedStreamConfig), anyhow::Error> {
        let available_hosts = cpal::available_hosts();
        let host_id = available_hosts
            .iter()
            .find(|item| host_name == item.name())
            .ok_or(anyhow!(
                "There is no audio host with name \"{}\"",
                host_name
            ))?;

        let host = cpal::host_from_id(*host_id)?;

        let device = if let Some(device_name) = device_name {
            host.devices()
                .context("Failed to get list of audio devices")?
                .into_iter()
                .find(|item| match item.name() {
                    Ok(name) => name == *device_name,
                    Err(_) => false,
                })
                .ok_or(anyhow!(
                    "There is no audio device named \"{}\"",
                    device_name
                ))?
        } else {
            host.default_output_device()
                .ok_or(anyhow!("Failed to get a default output device"))?
        };

        let config = if let Some(sample_config) = sample_config {
            device
                .supported_output_configs()
                .context("Failed to get supported output configs")?
                .into_iter()
                .find(|config: &SupportedStreamConfigRange| {
                    sample_config.sample_rate >= config.min_sample_rate().0
                        && sample_config.sample_rate <= config.max_sample_rate().0
                        && sample_config.channels == config.channels()
                        && sample_config.sample_format == config.sample_format()
                })
                .ok_or(anyhow!("Failed to find a supported output config"))?
                .with_sample_rate(SampleRate(sample_config.sample_rate))
        } else {
            device
                .default_output_config()
                .context("Failed to get a default output config")?
        };

        Ok((device, config))
    }

    fn get_input_device_config(
        host_name: &str,
        device_name: &Option<String>,
        sample_config: &Option<StreamConfig>,
    ) -> Result<(Device, SupportedStreamConfig), anyhow::Error> {
        let available_hosts = cpal::available_hosts();
        let host_id = available_hosts
            .iter()
            .find(|item| host_name == item.name())
            .ok_or(anyhow!(
                "There is no audio host with name \"{}\"",
                host_name
            ))?;

        let host = cpal::host_from_id(*host_id)?;

        let device = if let Some(device_name) = device_name {
            host.devices()
                .context("Failed to get list of audio devices")?
                .into_iter()
                .find(|item| match item.name() {
                    Ok(name) => name == *device_name,
                    Err(_) => false,
                })
                .ok_or(anyhow!(
                    "There is no audio device named \"{}\"",
                    device_name
                ))?
        } else {
            host.default_input_device()
                .ok_or(anyhow!("Failed to get a default input device"))?
        };

        let config = if let Some(sample_config) = sample_config {
            device
                .supported_input_configs()
                .context("Failed to get supported input configs")?
                .into_iter()
                .find(|config: &SupportedStreamConfigRange| {
                    sample_config.sample_rate >= config.min_sample_rate().0
                        && sample_config.sample_rate <= config.max_sample_rate().0
                        && sample_config.channels == config.channels()
                        && sample_config.sample_format == config.sample_format()
                })
                .ok_or(anyhow!("Failed to find a supported input config"))?
                .with_sample_rate(SampleRate(sample_config.sample_rate))
        } else {
            device
                .default_input_config()
                .context("Failed to get a default input config")?
        };

        Ok((device, config))
    }

    pub async fn receive_audio<T>(
        &self,
        sock_addr: &std::net::SocketAddr,
        device: &Device,
        supported_config: &SupportedStreamConfig,
    ) -> Result<(), anyhow::Error>
    where
        T: SizedSample + FromSample<f32>,
    {
        let config = supported_config.config();
        let channels = supported_config.channels() as usize;
        // TODO @john: Make audio buffer size configurable
        let audio_buffer: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(120000 * channels)));
        let audio_buffer_clone = audio_buffer.clone();
        let mut packet_buffer = vec![0u8; MTU];
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
                    info!("\nStopping...");
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
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .context("Failed to bind UDP socket and port")?;
        let mut sequence_number = 0u64;
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();
        let mut packet_buffer = Vec::with_capacity(MTU);
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

        loop {
            select! {
                _ = notify.notified() => {
                    // Drop through to handle below
                }
                _ = signal::ctrl_c() => {
                    info!("\nStopping...");
                    break;
                }
            }

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
                        && packet_buffer.len() + 2 * size_of::<f32>() <= MTU
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

        Ok(())
    }

    pub fn list(&self) -> Result<(), anyhow::Error> {
        fn format_config(
            config: &SupportedStreamConfigRange,
            default_config: &Option<SupportedStreamConfig>,
        ) -> String {
            fn format_as_khz(rate: u32) -> String {
                format!("{:.2}", rate as f32 / 1000.0)
                    .trim_end_matches("0")
                    .trim_end_matches(".")
                    .to_string()
            }
            format!(
                "\"{}x{}x{}\"{}",
                config.channels(),
                if config.min_sample_rate() == config.max_sample_rate() {
                    format!("{}", format_as_khz(config.max_sample_rate().0))
                } else {
                    format!(
                        "{}-{}",
                        format_as_khz(config.min_sample_rate().0),
                        format_as_khz(config.max_sample_rate().0)
                    )
                },
                config.sample_format().to_string(),
                if let Some(default_config) = default_config {
                    if default_config.channels() == config.channels()
                        && default_config.sample_rate() >= config.min_sample_rate()
                        && default_config.sample_rate() <= config.max_sample_rate()
                        && default_config.sample_format() == config.sample_format()
                    {
                        " (default)"
                    } else {
                        ""
                    }
                } else {
                    ""
                }
            )
        }

        let available_hosts = cpal::available_hosts();

        for host_id in available_hosts.iter() {
            let host = cpal::host_from_id(*host_id)?;

            println!("Host \"{}\"", host_id.name());

            let default_device_input_name = host
                .default_input_device()
                .map(|e| e.name().unwrap())
                .unwrap_or("".to_string());
            let default_device_output_name = host
                .default_output_device()
                .map(|e| e.name().unwrap())
                .unwrap_or("".to_string());
            let devices = host.devices()?;

            for device in devices {
                let device_name = device.name()?;

                println!(
                    "  Device \"{}\"{}{}",
                    device_name,
                    if device_name == default_device_input_name {
                        " (default input)"
                    } else {
                        ""
                    },
                    if device_name == default_device_output_name {
                        " (default output)"
                    } else {
                        ""
                    }
                );

                // Input configs
                let default_input_config = device.default_input_config().ok();
                let input_configs = match device.supported_input_configs() {
                    Ok(f) => f.collect(),
                    Err(_) => Vec::new(),
                };
                println!(
                    "    Input {}",
                    if input_configs.is_empty() {
                        "none".to_string()
                    } else {
                        input_configs
                            .into_iter()
                            .map(|config| format_config(&config, &default_input_config))
                            .collect::<Vec<String>>()
                            .join(", ")
                    }
                );

                // Output configs
                let default_output_config = device.default_output_config().ok();
                let output_configs = match device.supported_output_configs() {
                    Ok(f) => f.collect(),
                    Err(_) => Vec::new(),
                };
                println!(
                    "    Output {}",
                    if output_configs.is_empty() {
                        "none".to_string()
                    } else {
                        output_configs
                            .into_iter()
                            .map(|config| format_config(&config, &default_output_config))
                            .collect::<Vec<String>>()
                            .join(", ")
                    }
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn basic_test() {}
}
