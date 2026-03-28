use serde::{Deserialize, Serialize};

/// Connection configuration stored outside nih-plug's Params system
/// (which only supports numeric types) and serialized via Plugin::serialize_state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 7271,
        }
    }
}
