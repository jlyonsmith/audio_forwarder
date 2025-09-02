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
mod audio_caps;
mod client;
mod messages;
mod server;
mod stream_config;
mod udp_server;

pub use crate::client::Client;
pub use crate::server::Server;
pub use crate::stream_config::StreamConfig;
use std::time::Duration;

// TODO @john: Make this configurable
const MTU: usize = 65536;
const SERVER_TIMEOUT: Duration = Duration::from_secs(5);
