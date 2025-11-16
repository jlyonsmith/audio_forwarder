use crate::stream_config::StreamConfig;
use std::fmt::Display;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct DeviceConfig {
    pub direction: DeviceDirection,
    pub host_name: String,
    pub device_name: String,
    pub stream_cfg: StreamConfig,
}

// Example format: "Core Audio|Built-in Microphone|Input|2x44.1xf32"
impl Display for DeviceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}|{}|{}|{}",
            &self.host_name,
            &self.device_name,
            if &self.direction == &DeviceDirection::Input {
                "Input"
            } else {
                "Output"
            },
            self.stream_cfg
        )
    }
}

impl DeviceConfig {
    pub fn device_id(&self) -> String {
        format!(
            "{}|{}|{}",
            &self.host_name,
            &self.device_name,
            if &self.direction == &DeviceDirection::Input {
                "Input"
            } else {
                "Output"
            }
        )
    }
}
