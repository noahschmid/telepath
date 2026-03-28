use thiserror::Error;

#[derive(Debug, Error)]
pub enum FramingError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("message too large: {0} bytes")]
    MessageTooLarge(u32),
    #[error("decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
}

#[derive(Debug, Error)]
pub enum PacketError {
    #[error("datagram too short: {got} bytes")]
    TooShort { got: usize },
    #[error("truncated packet: expected {expected} bytes, got {got}")]
    Truncated { expected: usize, got: usize },
    #[error("too many frames: {0} (max {max})", max = crate::packet::MAX_FRAMES)]
    TooManyFrames(usize),
    #[error("encode buffer too small: need {need}, got {got}")]
    BufferTooSmall { need: usize, got: usize },
}
