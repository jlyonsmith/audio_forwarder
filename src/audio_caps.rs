use crate::device_config::{DeviceConfig, DeviceDirection};
pub use crate::stream_config::StreamConfig;
use anyhow::{anyhow, Context};
use cpal::{
    traits::{DeviceTrait, HostTrait},
    Device, SampleRate, SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange,
};
use std::fmt::Write;

pub struct AudioCaps {}

impl AudioCaps {
    pub fn get_output_device(
        host_name: &str,
        device_name: &Option<String>,
        stream_cfg: &Option<StreamConfig>,
    ) -> Result<(Device, DeviceConfig, u32), anyhow::Error> {
        let available_hosts = cpal::available_hosts();
        let host_id = available_hosts
            .iter()
            .find(|item| host_name == item.name())
            .ok_or(anyhow!(
                "There is no audio host with name \"{}\"",
                host_name
            ))?;

        let host = cpal::host_from_id(*host_id)?;

        let device = if let Some(device) = device_name {
            host.devices()
                .context("Failed to get list of audio devices")?
                .into_iter()
                .find(|item| match item.name() {
                    Ok(name) => name == *device,
                    Err(_) => false,
                })
                .ok_or(anyhow!("There is no audio device named \"{}\"", device))?
        } else {
            host.default_output_device()
                .ok_or(anyhow!("Failed to get a default output device"))?
        };

        let config = if let Some(stream_cfg) = stream_cfg {
            device
                .supported_output_configs()
                .context("Failed to get supported output configs")?
                .into_iter()
                .find(|config: &SupportedStreamConfigRange| {
                    stream_cfg.sample_rate >= config.min_sample_rate().0
                        && stream_cfg.sample_rate <= config.max_sample_rate().0
                        && stream_cfg.channels == config.channels()
                        && stream_cfg.sample_format == config.sample_format()
                })
                .ok_or(anyhow!("Failed to find a supported output config"))?
                .with_sample_rate(SampleRate(stream_cfg.sample_rate))
        } else {
            device
                .default_output_config()
                .context("Failed to get a default output config")?
        };

        Ok((
            device.clone(),
            DeviceConfig {
                direction: DeviceDirection::Output,
                host_name: host_id.name().to_string(),
                device_name: device.name()?,
                stream_cfg: StreamConfig {
                    channels: config.channels(),
                    sample_rate: config.sample_rate().0,
                    sample_format: config.sample_format(),
                },
            },
            match *config.buffer_size() {
                SupportedBufferSize::Range { min, max: _ } => min,
                SupportedBufferSize::Unknown => 1,
            },
        ))
    }

    pub fn get_input_device(
        host: &str,
        device: &Option<String>,
        stream_cfg: &Option<StreamConfig>,
    ) -> Result<(Device, DeviceConfig, u32), anyhow::Error> {
        let available_hosts = cpal::available_hosts();
        let host_id = available_hosts
            .iter()
            .find(|item| host == item.name())
            .ok_or(anyhow!("There is no audio host with name \"{}\"", host))?;

        let host = cpal::host_from_id(*host_id)?;

        let device = if let Some(device) = device {
            host.devices()
                .context("Failed to get list of audio devices")?
                .into_iter()
                .find(|item| match item.name() {
                    Ok(name) => name == *device,
                    Err(_) => false,
                })
                .ok_or(anyhow!("There is no audio device named \"{}\"", device))?
        } else {
            host.default_input_device()
                .ok_or(anyhow!("Failed to get a default input device"))?
        };

        let config = if let Some(stream_cfg) = stream_cfg {
            device
                .supported_input_configs()
                .context("Failed to get supported input configs")?
                .into_iter()
                .find(|config: &SupportedStreamConfigRange| {
                    stream_cfg.sample_rate >= config.min_sample_rate().0
                        && stream_cfg.sample_rate <= config.max_sample_rate().0
                        && stream_cfg.channels == config.channels()
                        && stream_cfg.sample_format == config.sample_format()
                })
                .ok_or(anyhow!("Failed to find a supported input config"))?
                .with_sample_rate(SampleRate(stream_cfg.sample_rate))
        } else {
            device
                .default_input_config()
                .context("Failed to get a default input config")?
        };

        Ok((
            device.clone(),
            DeviceConfig {
                direction: DeviceDirection::Input,
                host_name: host_id.name().to_string(),
                device_name: device.name()?,
                stream_cfg: StreamConfig {
                    channels: config.channels(),
                    sample_rate: config.sample_rate().0,
                    sample_format: config.sample_format(),
                },
            },
            match *config.buffer_size() {
                SupportedBufferSize::Range { min, max: _ } => min,
                SupportedBufferSize::Unknown => 1,
            },
        ))
    }

    pub fn get_device_list_string() -> anyhow::Result<String> {
        fn format_config(
            config: &SupportedStreamConfigRange,
            default_config: &Option<SupportedStreamConfig>,
        ) -> String {
            format!(
                "\"{}x{}x{}\"{}",
                config.channels(),
                if config.min_sample_rate() == config.max_sample_rate() {
                    format!(
                        "{}",
                        StreamConfig::format_as_khz(config.max_sample_rate().0)
                    )
                } else {
                    format!(
                        "{}-{}",
                        StreamConfig::format_as_khz(config.min_sample_rate().0),
                        StreamConfig::format_as_khz(config.max_sample_rate().0)
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

        let mut output = String::with_capacity(4096);
        let available_hosts = cpal::available_hosts();

        for host_id in available_hosts.iter() {
            let host = cpal::host_from_id(*host_id)?;

            writeln!(output, "Host \"{}\"", host_id.name())?;

            let default_device_input = host
                .default_input_device()
                .map(|e| e.name().unwrap())
                .unwrap_or("".to_string());
            let default_device_output = host
                .default_output_device()
                .map(|e| e.name().unwrap())
                .unwrap_or("".to_string());
            let devices = host.devices()?;

            for device in devices {
                let device_name = device.name()?;

                writeln!(
                    output,
                    "  Device \"{}\"{}{}",
                    device_name,
                    if device_name == default_device_input {
                        " (default input)"
                    } else {
                        ""
                    },
                    if device_name == default_device_output {
                        " (default output)"
                    } else {
                        ""
                    }
                )?;

                // Input configs
                let default_input_config = device.default_input_config().ok();
                let input_configs = match device.supported_input_configs() {
                    Ok(f) => f.collect(),
                    Err(_) => Vec::new(),
                };
                writeln!(
                    output,
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
                )?;

                // Output configs
                let default_output_config = device.default_output_config().ok();
                let output_configs = match device.supported_output_configs() {
                    Ok(f) => f.collect(),
                    Err(_) => Vec::new(),
                };
                writeln!(
                    output,
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
                )?;
            }
        }

        Ok(output)
    }
}
