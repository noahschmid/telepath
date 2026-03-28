use serde::{Deserialize, Serialize};

/// Current protocol version — both sides must agree or the session is rejected.
pub const PROTOCOL_VERSION: u16 = 1;

/// Default TCP port the host listens on.
pub const DEFAULT_PORT: u16 = 7271;

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

/// Sent by the host immediately after the plugin connects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHello {
    pub version: u16,
    /// Native sample rate of the selected capture device.
    pub sample_rate: u32,
    /// Number of channels being captured (always 1 for mono).
    pub channels: u8,
    /// Human-readable name of the selected audio device.
    pub device_name: String,
}

/// Sent by the plugin in response to ServerHello.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub version: u16,
    /// Sample rate the DAW is running at. If it differs from the device's
    /// native rate, the host should resample before sending.
    pub requested_sample_rate: u32,
    /// UDP port the plugin is listening on for incoming audio packets.
    /// Host sends all audio datagrams to plugin_ip:udp_listen_port.
    pub udp_listen_port: u16,
}

/// Sent by the host after validating ClientHello.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HandshakeResult {
    Ready,
    Error(SessionError),
}

// ---------------------------------------------------------------------------
// Session messages (after handshake)
// ---------------------------------------------------------------------------

/// Plugin → Host: control messages sent over TCP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginMessage {
    /// RTT probe — host must echo back a Pong immediately.
    Ping { seq: u32, timestamp_us: u64 },
    /// DAW transport is rolling and track is armed — host may start streaming.
    RecordStart,
    /// DAW transport stopped or track disarmed.
    RecordStop,
    /// Clean session teardown.
    Disconnect,
}

/// Host → Plugin: messages sent over TCP.
/// Named ClientMessage for historical reasons (host was called "client" earlier).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Echo of a Ping — timestamp is returned unchanged so the plugin can
    /// compute RTT as `now_us() - pong.timestamp_us`.
    Pong { seq: u32, timestamp_us: u64 },
    /// Periodic stream health report from the host.
    StreamStats {
        packets_sent: u64,
        packets_dropped: u32,
        /// Peak capture level in dBFS × 100 (e.g. -1800 = -18.00 dBFS).
        level_dbfs_x100: i16,
    },
    /// Host-side error (e.g. device disconnected).
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
            Self::VersionMismatch { got, expected } => {
                write!(
                    f,
                    "protocol version mismatch: got {got}, expected {expected}"
                )
            }
            Self::DeviceUnavailable => write!(f, "audio device unavailable"),
            Self::SampleRateMismatch { got, expected } => {
                write!(
                    f,
                    "sample rate mismatch: device {got} Hz, DAW {expected} Hz"
                )
            }
            Self::Internal(s) => write!(f, "internal error: {s}"),
        }
    }
}
