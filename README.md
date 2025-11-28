# Audio Forwarder

[![coverage](https://shields.io/endpoint?url=https://raw.githubusercontent.com/jlyonsmith/audio_forwarder/main/coverage.json)](https://github.com/jlyonsmith/audio_forwarder/blob/main/coverage.json)
[![Crates.io](https://img.shields.io/crates/v/audio_forwarder.svg)](https://crates.io/crates/audio_forwarder)
[![Docs.rs](https://docs.rs/audio_forwarder/badge.svg)](https://docs.rs/audio_forwarder)

<!-- cargo-rdme start -->

This program allows you to forward audio from one device to another over the network.
Audio can flow from a local input device to a remote output device, or from a remote input device
to a local output device.  Input devices are typically microphones, and output devices are typically
speakers.

You can also list the available audio devices on the local and remote machines.


To do this you must run one instance of the program in server mode. Then one or more client instances
can connect to this server instance to send or receive audio. To start a server instance
use the `--listen` flag followed by the address and port to listen on, e.g. `0.0.0.0:12345`.

The server is persistent and will continue to listen for incoming connections until it is stopped.
You can run the server as a `systemd` service on Linux, provided that the user running the service
has permission to access the audio devices.

Clients will remain connected until they are stopped, at which point they will disconnect from the server.
This frees up servers audio devices for other clients to use.

### Implementation Details

Audio transmission is done over UDP for performance and a separate TCP/IP connection is used
to controle and configure the audio stream.

Audio that is input from a 1-channel audio device will be written to the network as a
2-channel audio.

Network packets are always sent as 32-bit floating point values in the range of -1.0 to 1.0.

Audio stream configurations are specified in the format `<channels>x<khz>x<format>`

- `channels` - The number of channels in the audio stream. For example, 1 or 2.
- `khz` - The sample rate of the audio stream in kilohertz. For example, 44.1 or 48.
- `format` - The sample format of the audio stream. For example, f32, i16, u8.

The first letter of the `format` is the type of the data (`i`, `u` or `f`), and the second letter
is the number of bits per sample.  For example, f32 is a 32-bit floating point number, i16 is a 16-bit
signed integer, u8 is an 8-bit unsigned integer.

When listing the available audio devices, the format is `<channels>x<min-khz>-<max-khz>x<format>`. When
specifying a device configuration, you can use any single sample rate within the min-max range, e.g.
`2x48xf32`.

<!-- cargo-rdme end -->
