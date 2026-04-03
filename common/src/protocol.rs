use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const DEFAULT_PORT: u16 = 7271;

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

/// Sent by the host immediately after the receiver connects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHello {
    pub version: u16,
    pub sample_rate: u32,
    pub channels: u8,
    pub device_name: String,
    /// UDP port on the host that the receiver should send return audio to.
    pub return_udp_port: u16,
}

/// Sent by the receiver in response to ServerHello.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub version: u16,
    /// Sample rate the receiver's output device runs at.
    pub requested_sample_rate: u32,
    /// UDP port the receiver listens on for incoming audio packets.
    pub udp_listen_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HandshakeResult {
    Ready,
    Error(SessionError),
}

// ---------------------------------------------------------------------------
// Session messages
// ---------------------------------------------------------------------------

/// Receiver → Host (over TCP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReceiverMessage {
    Ping { seq: u32, timestamp_us: u64 },
    Disconnect,
}

/// Host → Receiver (over TCP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMessage {
    /// Echo of a Ping for RTT measurement.
    Pong {
        seq: u32,
        timestamp_us: u64,
    },
    StreamStats {
        packets_sent: u64,
        packets_dropped: u32,
        /// Capture level in dBFS × 100.
        level_dbfs_x100: i16,
    },
    Error(SessionError),
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionError {
    VersionMismatch { got: u16, expected: u16 },
    DeviceUnavailable,
    SampleRateMismatch { got: u32, expected: u32 },
    Internal(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionMismatch { got, expected } => write!(
                f,
                "protocol version mismatch: got {got}, expected {expected}"
            ),
            Self::DeviceUnavailable => write!(f, "audio device unavailable"),
            Self::SampleRateMismatch { got, expected } => write!(
                f,
                "sample rate mismatch: device {got} Hz, DAW {expected} Hz"
            ),
            Self::Internal(s) => write!(f, "internal error: {s}"),
        }
    }
}
