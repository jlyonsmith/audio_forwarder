use std::net::UdpSocket;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pargs = pico_args::Arguments::from_env();
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let server_addr: String = pargs.value_from_str("--server")?;
    let tone_frequency: f32 = pargs.value_from_str("--tone")?;

    println!("Sending test audio packets to {}", server_addr);

    let mut sequence: u64 = 1;

    // Generate some test audio data (sine wave)
    let sample_rate = 44100.0;
    let channels = 2;
    let mut sample_clock = 0f32;
    let volume = 1.0;
    let mut next_sample = move || {
        let sample = (sample_clock * tone_frequency * 2.0 * std::f32::consts::PI / sample_rate)
            .sin()
            * volume;
        sample_clock += 1.0;
        sample
    };
    let packet_duration = 0.1; // 100ms
    let samples_per_packet = (packet_duration * sample_rate) as usize;

    loop {
        // Create a test packet with sequence number + audio data
        let mut packet = Vec::new();

        // Add 64-bit sequence number (little-endian)
        packet.extend_from_slice(&sequence.to_le_bytes());

        println!(
            "Sending packet: seq={}, size={} bytes",
            sequence,
            packet.len()
        );

        // Add audio samples to the packet
        for _ in 0..samples_per_packet {
            let slice = next_sample().to_le_bytes();

            for _ in 0..channels {
                packet.extend_from_slice(&slice);
            }
        }

        // Send the packet
        match socket.send_to(&packet, &server_addr) {
            Ok(bytes_sent) => {
                println!("Sent packet: seq={}, size={} bytes", sequence, bytes_sent);
            }
            Err(e) => {
                eprintln!("Failed to send packet {}: {}", sequence, e);
            }
        }

        sequence += 1;

        // Add a little artificial delay between packets
        thread::sleep(Duration::from_millis(
            (1000.0 * packet_duration * 0.5) as u64,
        ));

        // Stop after 50 packets (5 seconds)
        if sequence > 50 {
            break;
        }
    }

    println!("Test client finished");
    Ok(())
}
