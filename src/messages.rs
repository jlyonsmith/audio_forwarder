use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum NetworkMessage {
    /// Request server to list available audio devices
    List,
    /// Response to List message
    ListResponse { output: String },
    /// Request server to read a local input audio device and send to a remote computer
    SendAudio {
        host: String,
        device: Option<String>,
        stream_cfg: Option<String>,
        udp_addr: String,
    },
    /// Response to SendAudio message
    SendAudioResponse {
        actual_host: String,
        actual_device: String,
        actual_stream_cfg: String,
    },
    /// Request server to receive audio from a remote computer and send to local output audio device
    ReceiveAudio {
        host: String,
        device: Option<String>,
        stream_cfg: Option<String>,
    },
    /// Response to ReceiveAudio message
    ReceiveAudioResponse {
        actual_host: String,
        actual_device: String,
        actual_stream_cfg: String,
        udp_addr: String,
    },
}
