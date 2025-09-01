use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum NetworkMessage {
    ListRemote,
    ListRemoteResponse { output: String },
}
