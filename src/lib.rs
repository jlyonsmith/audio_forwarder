//! This program allows you to forward audio from one device to another over the network.
//! Audio can be either sent to an ouput device, e.g. speakers, or received from an input device,
//! e.g. a microphone at either end of the connection.
//!
//! Audio transmission is done over UDP for performance and a separate TCP/IP connection is used
//! as a control channel and to configure the audio stream.
//!
//! To do this you must run one instance of the program in server mode. The one or more instances
//! can connect to the server instance in client mode to send or receive audio. To start a server instance
//! use the `--listen` flag followed by the address and port to listen on, e.g. `0.0.0.0:12345`.
//!
//! The server is persistent and will continue to listen for incoming connections until it is stopped.
//! You can run the server as a `systemd` service on Linux, provided that the user running the service
//! has permission to access the audio devices.
//!
//! Client instances will then connect to the server instance to send or receive audio. Clients will
//! remain connected until they are stopped, at which point they will disconnect from the server. This
//! frees up audio devices on the server for other clients to use.
//!
//! > Audio that is read from a 1-channel audio device will be written to the network as a 2-channel audio.
//!
//! Network packets are always sent as 32-bit floating point values in the range of -1.0 to 1.0.
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
//! When listing the available audio devices, the format is `<channels>x<max-khz>x<format>`.
//!
mod audio_caps;
mod client;
mod device_config;
mod messages;
mod server;
mod stream_config;
mod udp_server;

pub use crate::client::Client;
pub use crate::device_config::{DeviceConfig, DeviceDirection};
pub use crate::server::Server;
pub use crate::stream_config::StreamConfig;
use std::time::Duration;

// const MTU: usize = 65536;
const MTU: usize = 1472;
const SERVER_TIMEOUT: Duration = Duration::from_secs(5);
