# Audio Forwarder

[![coverage](https://shields.io/endpoint?url=https://raw.githubusercontent.com/jlyonsmith/audio_forwarder/main/coverage.json)](https://github.com/jlyonsmith/audio_forwarder/blob/main/coverage.json)
[![Crates.io](https://img.shields.io/crates/v/audio_forwarder.svg)](https://crates.io/crates/audio_forwarder)
[![Docs.rs](https://docs.rs/audio_forwarder/badge.svg)](https://docs.rs/audio_forwarder)

<!-- cargo-rdme start -->

## Usage

This program allows you to forward audio from one physical computer to another over the network.
Audio transmission can be from an input or output device on one computer to a corresponding output or
input device on the other computer.

First run one instance of the program in `listen` mode. Then one or more instances of the program can
connect to this audio server to `send` or `receive` audio. The server is persistent and will continue
to listen for incoming connections until it is stopped.

Senders or receivers remain connected until they are stopped, at which point they will disconnect from the
listening process. This frees up audio devices on the server for other clients to use.

This tool uses the [`cpal`](https://crates.io/crates/cpal) Rust crate to access the host audio devices.
CPAL defines audio using 3 parameters:

1. The audio _host_. For Macs this is **CoreAudio**, for Linux this is **ALSA**
2. The audio _device_.  For example, "MacBook Pro Microphone".
3. THe audio _stream config_.  This is a combination of channels, sample frequency and sample format (see below).

You must always specify the audio host, but the _device_ and _stream config_ will use defaults if not given.
The program can only forward 1 or 2 channel input signals on one side to 2 channel output on the other side.
The input and output frequencies must also match, as this program does not do up or down sampling. For this
reason using defaults may not always work and you probably want to be explicit.

You can connect multiple clients to one server using different audio host/device/stream config inputs or outputs.
Specifying a host/device/stream config that is currently in use will disconnect that stream and replace it with
the new one.

Finally, you can use the `list` or `list-remote` commands to find out what devices are available on the local
and remote machines.

## Implementation Details

You can run the server as a `systemd` service on Linux.  Make sure the user running the service has
permission to access the audio devices, e.g. `usermod -a -G audio <username>` on a Raspberry Pi.

Audio transmission is done over UDP for performance, and a TCP/IP connection is used
as a control channel and to configure the audio stream on the local and remote computers.

Audio samples are always transmitted as as 32-bit floating point values in the range of -1.0 to 1.0.

Audio that is read from a 1-channel audio input device will upsampled to 2-channel audio before transmission
over the network.

Audio stream configurations are specified on the command line in the format `<channels>x<khz>x<format>`

- `channels` - The number of channels in the audio stream. 1 or 2 for input, 2 for output.
- `khz` - The sample rate of the audio stream in kilohertz. For example, 44.1 or 48.
- `format` - The sample format of the audio stream. Only f32 and i32 are supported currently.

> The first letter of the `format` is the type of the data (`i`, `u` or `f`), and the second letter
> is the number of bits per sample.  For example, f32 is a 32-bit floating point number.

Packets are numbered, so audio will not be garbled if packets are delivered out of sequence, but you
will hear dropouts.  Using over WiFi will not give high quality audio; use a wired network for best results.

This system is designed for ease of use and is _not_ a secure way to transmit audio.  Do not use outside of a
firewalled private network.

<!-- cargo-rdme end -->
