use crate::device_config::{DeviceConfig, DeviceDirection};
pub use crate::stream_config::StreamConfig;
use anyhow::{anyhow, bail, Context};
use cpal::{
    traits::{DeviceTrait, HostTrait},
    Device, SampleRate, SupportedStreamConfig, SupportedStreamConfigRange,
};
use std::{fmt::Write, vec};

pub struct AudioCaps {}

impl AudioCaps {
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

        eprintln!("Using audio host: \"{}\"", host_name);

        let host = cpal::host_from_id(*host_id)?;
        let devices;

        if let Some(device) = device_name {
            devices = host
                .devices()
                .context("Failed to get full list of audio devices")?
                .into_iter()
                .filter(|item| match item.name() {
                    Ok(name) => name == *device,
                    Err(_) => false,
                })
                .collect::<Vec<Device>>();

            if devices.is_empty() {
                bail!(
                    "There is no audio output device named \"{}\"",
                    device_name.as_ref().unwrap()
                );
            }
        } else {
            devices = vec![host
                .default_output_device()
                .ok_or(anyhow!("Failed to get a default output device"))?];
        };

        // cpal may return multiple devices with the same name, so iterate through all
        // matching devices and find one that supports the requested stream config.
        for device in devices {
            eprintln!(
                "Checking output device: \"{}\"",
                device.name().unwrap_or_default()
            );
            let config: Option<SupportedStreamConfig> = if let Some(stream_cfg) = stream_cfg {
                eprintln!(
                    "Requested output stream config: {} channels, {} Hz, {:?}\n",
                    stream_cfg.channels, stream_cfg.sample_rate, stream_cfg.sample_format
                );
                device
                    .supported_output_configs()
                    .context(anyhow!(
                        "Failed to get all supported output stream configs for device \"{}\"",
                        device.name().unwrap_or_default()
                    ))?
                    .into_iter()
                    .find(|config: &SupportedStreamConfigRange| {
                        eprintln!(
                            "Checking output config: {} channels, min {} Hz, max {} Hz, {:?}",
                            config.channels(),
                            config.min_sample_rate().0,
                            config.max_sample_rate().0,
                            config.sample_format()
                        );
                        stream_cfg.sample_rate >= config.min_sample_rate().0
                            && stream_cfg.sample_rate <= config.max_sample_rate().0
                            && stream_cfg.channels == config.channels()
                            && stream_cfg.sample_format == config.sample_format()
                    })
                    .map(|range| range.with_sample_rate(SampleRate(stream_cfg.sample_rate)))
            } else {
                device
                    .default_output_config()
                    .context(anyhow!(
                        "Failed to get a default output stream config for device \"{}\"",
                        device.name().unwrap_or_default()
                    ))
                    .ok()
            };

            if let Some(config) = config {
                return Ok((
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
                ));
            }
        }

        bail!("Failed to find a suitable output device");
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
        let devices;

        if let Some(device) = device_name {
            devices = host
                .devices()
                .context("Failed to get full list of audio devices")?
                .into_iter()
                .filter(|item| match item.name() {
                    Ok(name) => name == *device,
                    Err(_) => false,
                })
                .collect::<Vec<Device>>();

            if devices.is_empty() {
                bail!(
                    "There is no audio input device named \"{}\"",
                    device_name.as_ref().unwrap()
                );
            }
        } else {
            devices = vec![host
                .default_input_device()
                .ok_or(anyhow!("Failed to get a default input device"))?];
        };

        // cpal may return multiple devices with the same name, so iterate through all
        // matching devices and find one that supports the requested stream config.
        for device in devices {
            let config: Option<SupportedStreamConfig> = if let Some(stream_cfg) = stream_cfg {
                device
                    .supported_input_configs()
                    .context("Failed to get supported input stream configs")?
                    .into_iter()
                    .find(|config: &SupportedStreamConfigRange| {
                        stream_cfg.sample_rate >= config.min_sample_rate().0
                            && stream_cfg.sample_rate <= config.max_sample_rate().0
                            && stream_cfg.channels == config.channels()
                            && stream_cfg.sample_format == config.sample_format()
                    })
                    .map(|range| range.with_sample_rate(SampleRate(stream_cfg.sample_rate)))
            } else {
                device
                    .default_input_config()
                    .context(anyhow!(
                        "Failed to get a default input stream config for device \"{}\"",
                        device.name().unwrap_or_default()
                    ))
                    .ok()
            };

            if let Some(config) = config {
                return Ok((
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
                ));
            }
        }

        bail!("Failed to find a suitable input device");
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
            let mut device_names: Vec<String> = vec![];
            let mut device_lines: Vec<String> = vec![];
            let mut input_lines: Vec<String> = vec![];
            let mut output_lines: Vec<String> = vec![];

            for ref device in host.devices()? {
                let device_name = device.name()?;

                let device_line = format!(
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
                );

                // Input configs
                let default_input_stream_cfg = device.default_input_config().ok();
                let input_stream_cfgs_list = match device.supported_input_configs() {
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
                let input_stream_cfg_line = format!(
                    "    Input {}",
                    if input_stream_cfgs_list.is_empty() {
                        "none".to_string()
                    } else {
                        input_stream_cfgs_list.join(", ")
                    }
                );

                // Output configs
                let default_output_stream_cfg = device.default_output_config().ok();
                let output_stream_cfgs_list = match device.supported_output_configs() {
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
                let output_stream_cfg_line = format!(
                    "    Output {}",
                    if output_stream_cfgs_list.is_empty() {
                        "none".to_string()
                    } else {
                        output_stream_cfgs_list.join(", ")
                    }
                );

                // cpal will sometimes return an input and output device with the same name.
                // The user doesn't care about that distinction when listing devices so we
                // merge the info into a single entry.
                if let Some(index) = device_names.iter().position(|name| *name == device_name) {
                    input_lines[index] = if input_stream_cfg_line.len() > input_lines[index].len() {
                        input_stream_cfg_line
                    } else {
                        input_lines[index].clone()
                    };
                    output_lines[index] =
                        if output_stream_cfg_line.len() > output_lines[index].len() {
                            output_stream_cfg_line
                        } else {
                            output_lines[index].clone()
                        };
                } else {
                    device_names.push(device_name);
                    device_lines.push(device_line);
                    input_lines.push(input_stream_cfg_line);
                    output_lines.push(output_stream_cfg_line);
                }
            }

            for i in 0..device_names.len() {
                writeln!(output, "{}", device_lines[i])?;
                writeln!(output, "{}", input_lines[i])?;
                writeln!(output, "{}", output_lines[i])?;
            }
        }

        Ok(output)
    }
}
