use cpal::SampleFormat;
use std::{fmt::Display, str::FromStr};

#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: SampleFormat,
}

impl FromStr for StreamConfig {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<StreamConfig, anyhow::Error> {
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

impl StreamConfig {
    pub fn format_as_khz(rate: u32) -> String {
        format!("{:.2}", rate as f32 / 1000.0)
            .trim_end_matches("0")
            .trim_end_matches(".")
            .to_string()
    }
}

impl Display for StreamConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}x{}x{}",
            self.channels,
            Self::format_as_khz(self.sample_rate),
            self.sample_format
        )
    }
}
