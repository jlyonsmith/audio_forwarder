use crate::device_config::{DeviceConfig, DeviceDirection};
pub use crate::stream_config::StreamConfig;
use anyhow::{anyhow, Context};
use cpal::{
    traits::{DeviceTrait, HostTrait},
    Device, SampleRate, SupportedStreamConfig, SupportedStreamConfigRange,
};
use std::fmt::Write;

pub struct AudioCaps {}

impl AudioCaps {
    pub fn make_strings_unique(mut strings: Vec<String>) -> Vec<String> {
        use std::collections::HashMap;

        let mut seen_counts: HashMap<String, usize> = HashMap::new();

        for string in strings.iter() {
            seen_counts
                .entry(string.clone())
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }

        let duplicates: Vec<bool> = strings
            .iter()
            .map(|s| *seen_counts.get(s).unwrap() > 1)
            .collect();

        let mut seen_counts: HashMap<String, usize> = HashMap::new();

        for (index, string) in strings.iter_mut().enumerate() {
            let count = seen_counts
                .entry(string.clone())
                .and_modify(|count| *count += 1)
                .or_insert(1);

            if duplicates[index] && *count > 0 {
                *string = format!("{} #{}", string, *count);
            }
        }

        strings
    }

    fn get_device(
        device_name: &Option<String>,
        host_id: &cpal::HostId,
        host: cpal::Host,
    ) -> Result<Device, anyhow::Error> {
        let device: Device = if let Some(device_name) = device_name {
            let device_index = Self::make_strings_unique(
                host.devices()
                    .context(anyhow!(
                        "Failed to get list of audio devices on host {}",
                        host_id.name()
                    ))?
                    .into_iter()
                    .filter_map(|device| device.name().ok())
                    .collect::<Vec<String>>(),
            )
            .iter()
            .position(|s| s == device_name)
            .ok_or(anyhow!(
                "There is no audio device named \"{}\"",
                device_name
            ))?;

            host.devices()?.into_iter().nth(device_index).unwrap()
        } else {
            host.default_output_device()
                .ok_or(anyhow!("Failed to get a default device"))?
        };
        Ok(device)
    }

    pub fn get_output_device(
        host_name: &str,
        device_name: &Option<String>,
        stream_cfg: &Option<StreamConfig>,
    ) -> Result<(Device, DeviceConfig), anyhow::Error> {
        let available_hosts = cpal::available_hosts();
        let host_id = available_hosts
            .iter()
            .find(|item| host_name == item.name())
            .ok_or(anyhow!(
                "There is no audio host with name \"{}\"",
                host_name
            ))?;

        let host = cpal::host_from_id(*host_id)?;
        let device = Self::get_device(device_name, host_id, host)?;
        let config = if let Some(stream_cfg) = stream_cfg {
            device
                .supported_output_configs()
                .context("Failed to get supported output stream configs")?
                .into_iter()
                .find(|config: &SupportedStreamConfigRange| {
                    stream_cfg.sample_rate >= config.min_sample_rate().0
                        && stream_cfg.sample_rate <= config.max_sample_rate().0
                        && stream_cfg.channels == config.channels()
                        && stream_cfg.sample_format == config.sample_format()
                })
                .ok_or(anyhow!("Failed to find a supported output stream config"))?
                .with_sample_rate(SampleRate(stream_cfg.sample_rate))
        } else {
            device
                .default_output_config()
                .context("Failed to get a default output stream config")?
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
        ))
    }

    pub fn get_input_device(
        host: &str,
        device_name: &Option<String>,
        stream_cfg: &Option<StreamConfig>,
    ) -> Result<(Device, DeviceConfig), anyhow::Error> {
        let available_hosts = cpal::available_hosts();
        let host_id = available_hosts
            .iter()
            .find(|item| host == item.name())
            .ok_or(anyhow!("There is no audio host with name \"{}\"", host))?;

        let host = cpal::host_from_id(*host_id)?;
        let device = Self::get_device(device_name, host_id, host)?;
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
        ))
    }

    pub fn get_device_list_string() -> anyhow::Result<String> {
        fn format_config(
            stream_cfg: &SupportedStreamConfigRange,
            default_stream_cfg: &Option<SupportedStreamConfig>,
        ) -> String {
            format!(
                "\"{}x{}x{}\"{}",
                stream_cfg.channels(),
                if stream_cfg.min_sample_rate() == stream_cfg.max_sample_rate() {
                    format!(
                        "{}",
                        StreamConfig::format_as_khz(stream_cfg.max_sample_rate().0)
                    )
                } else {
                    format!(
                        "{}-{}",
                        StreamConfig::format_as_khz(stream_cfg.min_sample_rate().0),
                        StreamConfig::format_as_khz(stream_cfg.max_sample_rate().0)
                    )
                },
                stream_cfg.sample_format().to_string(),
                if let Some(default_stream_cfg) = default_stream_cfg {
                    if default_stream_cfg.channels() == stream_cfg.channels()
                        && default_stream_cfg.sample_rate() >= stream_cfg.min_sample_rate()
                        && default_stream_cfg.sample_rate() <= stream_cfg.max_sample_rate()
                        && default_stream_cfg.sample_format() == stream_cfg.sample_format()
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

            let device_names = Self::make_strings_unique(
                host.devices()
                    .context(anyhow!(
                        "Failed to get list of audio devices on host {}",
                        host_id.name()
                    ))?
                    .into_iter()
                    .filter_map(|device| device.name().ok())
                    .collect::<Vec<String>>(),
            );

            for (index, device) in devices.enumerate() {
                let device_name = device_names[index].clone();

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
                let default_input_stream_cfg = device.default_input_config().ok();
                let input_stream_cfgs = match device.supported_input_configs() {
                    Ok(f) => f
                        .filter(|config| {
                            (config.channels() == 1 || config.channels() == 2)
                                && (config.sample_format() == cpal::SampleFormat::F32
                                    || config.sample_format() == cpal::SampleFormat::I32)
                        })
                        .map(|config| format_config(&config, &default_input_stream_cfg))
                        .collect::<Vec<String>>(),
                    Err(_) => Vec::new(),
                };
                writeln!(
                    output,
                    "    Input {}",
                    if input_stream_cfgs.is_empty() {
                        "none".to_string()
                    } else {
                        input_stream_cfgs.join(", ")
                    }
                )?;

                // Output configs
                let default_output_stream_cfg = device.default_output_config().ok();
                let output_stream_cfgs = match device.supported_output_configs() {
                    Ok(f) => f
                        .filter(|config| {
                            config.channels() == 2
                                && (config.sample_format() == cpal::SampleFormat::F32
                                    || config.sample_format() == cpal::SampleFormat::I32)
                        })
                        .map(|config| format_config(&config, &default_output_stream_cfg))
                        .collect::<Vec<String>>(),
                    Err(_) => Vec::new(),
                };

                writeln!(
                    output,
                    "    Output {}",
                    if output_stream_cfgs.is_empty() {
                        "none".to_string()
                    } else {
                        output_stream_cfgs.join(", ")
                    }
                )?;
            }
        }

        Ok(output)
    }
}
