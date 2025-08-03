mod log_macros;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use core::fmt::Arguments;
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, FromSample, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat, SizedSample,
    SupportedStreamConfig, SupportedStreamConfigRange,
};
use ctrlc;
use simple_cancelation_token::CancelationToken;
use std::{
    collections::VecDeque,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    sync::{Arc, Mutex},
    time::Duration,
};
use termion::color;

pub trait AudioForwarderLog: Send + Sync {
    fn output(self: &Self, args: Arguments);
    fn warning(self: &Self, args: Arguments);
    fn error(self: &Self, args: Arguments);
}

pub struct AudioForwarderTool {
    log: Arc<dyn AudioForwarderLog>,
}

#[derive(Debug, Clone)]
struct StreamConfig {
    channels: u16,
    sample_rate: u32,
}

impl StreamConfig {
    fn parse(s: &str) -> Result<StreamConfig, anyhow::Error> {
        let mut parts = s.split('x');
        let channels = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing channels"))?
            .parse()?;
        let sample_rate: f32 = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing sample rate"))?
            .parse()?;
        Ok(StreamConfig {
            channels,
            sample_rate: (sample_rate * 1000.0) as u32,
        })
    }
}

#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
/// Audio Forwarder Tool
///
/// This tool allows you to forward audio from one device to another over the network.
/// Audio can be read from the network and written to a device, or it can be read from
/// a device and written out to the network.  Which one depends on whether you supply
/// the `receive` or `send` arguments.
///
/// Audio that is read from a 1-channel audio device will be written to the network
/// as a 2-channel audio if the `mono-to-stereo` flag is set.
///
/// Network packets are sent as 32-bit floating point values in the range of -1.0 to 1.0.
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Disable colors in output
    #[arg(long = "no-color", short = 'n', env = "NO_CLI_COLOR")]
    no_color: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    List,
    Receive(ReceiveArgs),
    Send(SendArgs),
}

#[derive(Parser, Debug)]
struct ReceiveArgs {
    /// The name of the audio host
    #[arg(long = "host")]
    host_name: String,

    /// The name of the audio device
    #[arg(long = "device")]
    device_name: Option<String>,

    /// The audio device stream configuration in the format "<channels>x<sample_rate>"
    #[arg(long = "config", value_parser = StreamConfig::parse)]
    stream_config: Option<StreamConfig>,

    /// The IP address and port to listen on for incoming audio
    #[arg(long = "addr")]
    sock_addr: SocketAddr,

    /// The IP address to allow receiving audio from
    #[arg(long = "allow")]
    allow_addr: Option<SocketAddr>,
}

#[derive(Parser, Debug)]
struct SendArgs {
    /// The name of the audio host
    #[arg(long = "host")]
    host_name: String,

    /// The name of the audio device
    #[arg(long = "device")]
    device_name: Option<String>,

    /// The audio device stream configuration in the format "<channels>x<sample_rate>"
    #[arg(long = "config", value_parser = StreamConfig::parse)]
    stream_config: Option<StreamConfig>,

    /// The IP address and port to send audio to
    #[arg(long = "addr")]
    sock_addr: SocketAddr,

    /// Convert mono audio to stereo audio before sending
    #[arg(long = "mono-to-stereo")]
    mono_to_stereo: bool,
}

impl<'a> AudioForwarderTool {
    pub fn new(log: Arc<dyn AudioForwarderLog>) -> AudioForwarderTool {
        AudioForwarderTool { log }
    }

    pub fn run(
        self: &mut Self,
        args: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> Result<(), anyhow::Error> {
        let cli = match Cli::try_parse_from(args) {
            Ok(m) => m,
            Err(err) => {
                output!(self.log, "{}", err.to_string());
                return Ok(());
            }
        };

        match cli.command {
            Commands::List => {
                self.list_devices()?;
            }
            Commands::Receive(args) => {
                let (device, config) = Self::get_output_device_config(
                    &args.host_name,
                    args.device_name,
                    args.stream_config,
                )?;

                match config.sample_format() {
                    SampleFormat::F32 => {
                        self.receive_audio::<f32>(&args.sock_addr, &device, &config)?
                    }
                    SampleFormat::I16 => {
                        self.receive_audio::<i16>(&args.sock_addr, &device, &config)?
                    }
                    SampleFormat::U16 => {
                        self.receive_audio::<u16>(&args.sock_addr, &device, &config)?
                    }
                    _ => panic!("Unsupported sample format"),
                }
            }
            Commands::Send(args) => {
                let (device, config) = Self::get_input_device_config(
                    &args.host_name,
                    args.device_name,
                    args.stream_config,
                )?;

                match config.sample_format() {
                    SampleFormat::F32 => {
                        self.send_audio::<f32>(&args.sock_addr, &device, &config)?
                    }
                    SampleFormat::I16 => {
                        self.send_audio::<i16>(&args.sock_addr, &device, &config)?
                    }
                    SampleFormat::U16 => {
                        self.send_audio::<u16>(&args.sock_addr, &device, &config)?
                    }
                    _ => panic!("Unsupported sample format"),
                }
            }
        }

        Ok(())
    }

    fn get_output_device_config(
        host_name: &str,
        device_name: Option<String>,
        sample_config: Option<StreamConfig>,
    ) -> Result<(Device, SupportedStreamConfig), anyhow::Error> {
        let available_hosts = cpal::available_hosts();
        let host_id = available_hosts
            .iter()
            .find(|item| host_name == item.name())
            .ok_or(anyhow!("There is no audio host with name '{}'", host_name))?;

        let host = cpal::host_from_id(*host_id)?;

        let device = if let Some(device_name) = device_name {
            host.devices()
                .context("Failed to get list of audio devices")?
                .into_iter()
                .find(|item| match item.name() {
                    Ok(name) => name == device_name,
                    Err(_) => false,
                })
                .ok_or(anyhow!("There is no audio device named '{}'", device_name))?
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
                    sample_config.sample_rate == config.max_sample_rate().0
                        && sample_config.channels == config.channels()
                })
                .ok_or(anyhow!("Failed to find a supported output config"))?
                .with_max_sample_rate()
        } else {
            device
                .default_output_config()
                .context("Failed to get a default output config")?
        };

        Ok((device, config))
    }

    fn get_input_device_config(
        host_name: &str,
        device_name: Option<String>,
        sample_config: Option<StreamConfig>,
    ) -> Result<(Device, SupportedStreamConfig), anyhow::Error> {
        let available_hosts = cpal::available_hosts();
        let host_id = available_hosts
            .iter()
            .find(|item| host_name == item.name())
            .ok_or(anyhow!("There is no audio host with name '{}'", host_name))?;

        let host = cpal::host_from_id(*host_id)?;

        let device = if let Some(device_name) = device_name {
            host.devices()
                .context("Failed to get list of audio devices")?
                .into_iter()
                .find(|item| match item.name() {
                    Ok(name) => name == device_name,
                    Err(_) => false,
                })
                .ok_or(anyhow!("There is no audio device named '{}'", device_name))?
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
                    sample_config.sample_rate == config.max_sample_rate().0
                        && sample_config.channels == config.channels()
                })
                .ok_or(anyhow!("Failed to find a supported input config"))?
                .with_max_sample_rate()
        } else {
            device
                .default_input_config()
                .context("Failed to get a default input config")?
        };

        Ok((device, config))
    }

    pub fn receive_audio<T>(
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
        let audio_buffer: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(8192 * channels)));
        let log_clone = self.log.clone();
        let audio_buffer_clone = audio_buffer.clone();
        let stream = device.build_output_stream(
            &config,
            move |output: &mut [T], _: &OutputCallbackInfo| {
                let mut audio_buffer = audio_buffer_clone.lock().unwrap();

                for sample in output.iter_mut() {
                    if let Some(value) = audio_buffer.pop_front() {
                        *sample = T::from_sample(value);
                    } else {
                        *sample = Sample::EQUILIBRIUM;
                    }
                }
            },
            move |err| {
                error!(log_clone, "Audio stream error - {}", err);
            },
            None,
        )?;

        let mut packet_buffer = vec![0u8; 65536];
        let socket = UdpSocket::bind(sock_addr).context("Failed to bind UDP socket")?;

        socket.set_read_timeout(Some(Duration::from_millis(1000)))?;

        let token = CancelationToken::new();
        let token_clone = token.clone();
        let log_clone = self.log.clone();

        ctrlc::set_handler(move || {
            output!(log_clone, "\nStopping...");
            token_clone.cancel();
            ()
        })?;

        let mut last_sequence_number = 0u64;

        stream.play()?;

        output!(self.log, "Receiving audio on address {}", sock_addr);

        loop {
            match socket.recv_from(&mut packet_buffer) {
                Ok((_len, _addr)) => {}
                Err(e) => match e.kind() {
                    ErrorKind::WouldBlock => {}
                    other_error => {
                        error!(self.log, "Failed to receive audio packet - {}", other_error);
                    }
                },
            }

            if token.is_canceled() {
                break;
            }

            let sequence_number;

            if packet_buffer.len() >= size_of::<u64>() {
                sequence_number =
                    u64::from_le_bytes(packet_buffer[..size_of::<u64>()].try_into().unwrap());
            } else {
                // Packet too small
                continue;
            }

            if sequence_number <= last_sequence_number {
                // Out of sequence packet
                continue;
            }

            last_sequence_number = sequence_number;

            // Extract audio data
            let audio_data = &packet_buffer[size_of::<u64>()..];

            // Convert bytes to samples
            if audio_data.len() % size_of::<f32>() != 0 {
                // Audio data length not divisible by sample size
                continue;
            }

            {
                let mut audio_buffer = audio_buffer.lock().unwrap();
                let chunks = audio_data.chunks_exact(size_of::<f32>());

                // Prevent buffer overflow
                while audio_buffer.len() + chunks.len() > audio_buffer.capacity() {
                    audio_buffer.pop_front();
                }

                for chunk in chunks {
                    let sample = f32::from_le_bytes(chunk.try_into().unwrap());
                    audio_buffer.push_back(sample);
                }
            }
        }

        Ok(())
    }

    pub fn send_audio<T>(
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
        let audio_buffer: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(8192 * channels)));
        let log_clone = self.log.clone();
        let audio_buffer_clone = audio_buffer.clone();
        let stream = device.build_input_stream(
            &config,
            move |input: &[T], _: &InputCallbackInfo| {
                let mut _audio_buffer = audio_buffer_clone.lock().unwrap();

                for _sample in input.iter() {}
            },
            move |err| {
                error!(log_clone, "Audio stream error - {}", err);
            },
            None,
        )?;

        let mut packet_buffer = vec![0u8; 65536];
        let socket = UdpSocket::bind(sock_addr).context("Failed to bind UDP socket and port")?;

        socket.set_write_timeout(Some(Duration::from_millis(1000)))?;

        let token = CancelationToken::new();
        let token_clone = token.clone();
        let log_clone = self.log.clone();

        ctrlc::set_handler(move || {
            output!(log_clone, "\nStopping...");
            token_clone.cancel();
            ()
        })?;

        let mut last_sequence_number = 1u64;

        stream.play()?;

        output!(self.log, "Sending audio to address {}", sock_addr);

        loop {
            match socket.send_to(&mut packet_buffer, sock_addr) {
                Ok(_) => {}
                Err(e) => match e.kind() {
                    ErrorKind::WouldBlock => {}
                    other_error => {
                        error!(self.log, "Failed to send audio packet - {}", other_error);
                    }
                },
            }

            if token.is_canceled() {
                break;
            }

            let sequence_number;

            if packet_buffer.len() >= size_of::<u64>() {
                sequence_number =
                    u64::from_le_bytes(packet_buffer[..size_of::<u64>()].try_into().unwrap());
            } else {
                // Packet too small
                continue;
            }

            if sequence_number <= last_sequence_number {
                // Out of sequence packet
                continue;
            }

            last_sequence_number = sequence_number;

            // Extract audio data
            let audio_data = &packet_buffer[size_of::<u64>()..];

            // Convert bytes to samples
            if audio_data.len() % size_of::<f32>() != 0 {
                // Audio data length not divisible by sample size
                continue;
            }

            {
                let mut audio_buffer = audio_buffer.lock().unwrap();
                let chunks = audio_data.chunks_exact(size_of::<f32>());

                // Prevent buffer overflow
                while audio_buffer.len() + chunks.len() > audio_buffer.capacity() {
                    audio_buffer.pop_front();
                }

                for chunk in chunks {
                    let sample = f32::from_le_bytes(chunk.try_into().unwrap());
                    audio_buffer.push_back(sample);
                }
            }
        }

        Ok(())
    }

    pub fn list_devices(&self) -> Result<(), anyhow::Error> {
        fn format_config(
            is_input: bool,
            config: &SupportedStreamConfigRange,
            default_config: &Option<SupportedStreamConfig>,
        ) -> String {
            format!(
                "{} \"{}x{}\"{} ({} KHz to {} KHz, {} {}, {}){}{}",
                if is_input { "Input" } else { "Output" },
                config.channels(),
                config.max_sample_rate().0 as f32 / 1000.0,
                color::Fg(color::Rgb(64, 64, 64)),
                config.min_sample_rate().0 as f32 / 1000.0,
                config.max_sample_rate().0 as f32 / 1000.0,
                config.channels(),
                if config.channels() > 1 {
                    "channels"
                } else {
                    "channel"
                },
                match config.sample_format() {
                    SampleFormat::I8 => "8 bit signed integer",
                    SampleFormat::I16 => "16 bit signed integer",
                    SampleFormat::I32 => "32 bit signed integer",
                    SampleFormat::I64 => "32 bit signed integer",
                    SampleFormat::U8 => "8 bit unsigned integer",
                    SampleFormat::U32 => "32 bit unsigned integer",
                    SampleFormat::U64 => "64 bit unsigned integer",
                    SampleFormat::F32 => "32 bit float",
                    SampleFormat::F64 => "64 bit float",
                    _ => "unknown",
                },
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
                },
                color::Fg(color::Reset)
            )
        }

        let available_hosts = cpal::available_hosts();

        for host_id in available_hosts.iter() {
            let host = cpal::host_from_id(*host_id)?;

            output!(self.log, "Host \"{:?}\"", host_id);

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

                output!(
                    self.log,
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
                    Err(e) => {
                        error!(self.log, "Unable to get input configs - {:?}", e);
                        Vec::new()
                    }
                };
                for config in input_configs.into_iter() {
                    output!(
                        self.log,
                        "    {}",
                        format_config(true, &config, &default_input_config)
                    );
                }

                // Output configs
                let default_output_config = device.default_output_config().ok();
                let output_configs = match device.supported_output_configs() {
                    Ok(f) => f.collect(),
                    Err(e) => {
                        error!(self.log, "Unable to get supported output configs - {:?}", e);
                        Vec::new()
                    }
                };
                for config in output_configs.into_iter() {
                    output!(
                        self.log,
                        "    {}",
                        format_config(false, &config, &default_output_config)
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_test() {
        struct TestLogger;

        impl TestLogger {
            fn new() -> TestLogger {
                TestLogger {}
            }
        }

        impl AudioForwarderLog for TestLogger {
            fn output(self: &Self, _args: Arguments) {}
            fn warning(self: &Self, _args: Arguments) {}
            fn error(self: &Self, _args: Arguments) {}
        }

        let logger = Arc::new(TestLogger::new());
        let mut tool = AudioForwarderTool::new(logger);
        let args: Vec<std::ffi::OsString> = vec!["".into(), "--help".into()];

        tool.run(args).unwrap();
    }
}
