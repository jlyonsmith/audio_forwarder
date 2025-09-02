use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum NetworkMessage {
    /// Request server to list available audio devices
    List,
    /// Response to List message
    ListResponse { output: String },
    /// Request server to send audio to a host
    SendAudio {
        host: String,
        device: Option<String>,
        stream_config: Option<String>,
    },
    /// Response to SendAudio message
    SendAudioResponse {
        host: String,
        device: String,
        stream_config: String,
        udp_addr: String,
    },
    /// Request server to receive audio from a host
    ReceiveAudio {
        host: String,
        device: Option<String>,
        stream_config: Option<String>,
    },
    /// Response to ReceiveAudio message
    ReceiveAudioResponse {
        host: String,
        device: String,
        stream_config: String,
        udp_addr: String,
    },
}
