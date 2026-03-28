//! Length-prefixed MessagePack framing for the TCP control channel.
//!
//! Wire format per message:
//!   [length: u32 LE][msgpack payload: length bytes]
//!
//! Works over any `AsyncReadExt + AsyncWriteExt` — typically a
//! `tokio::net::TcpStream` split into read/write halves.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::FramingError;

/// Maximum allowed message size (4 MiB). Protects against malformed lengths.
const MAX_MESSAGE_BYTES: u32 = 4 * 1024 * 1024;

/// Read one length-prefixed MessagePack message from `reader` and deserialize it.
pub async fn read_message<R, T>(reader: &mut R) -> Result<T, FramingError>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    // Read 4-byte length prefix
    let len = reader.read_u32_le().await?;
    if len > MAX_MESSAGE_BYTES {
        return Err(FramingError::MessageTooLarge(len));
    }

    // Read payload
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).await?;

    // Deserialize
    rmp_serde::from_slice(&buf).map_err(FramingError::Decode)
}

/// Serialize `message` as MessagePack and write it length-prefixed to `writer`.
pub async fn write_message<W, T>(writer: &mut W, message: &T) -> Result<(), FramingError>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let payload = rmp_serde::to_vec_named(message).map_err(FramingError::Encode)?;
    if payload.len() > MAX_MESSAGE_BYTES as usize {
        return Err(FramingError::MessageTooLarge(payload.len() as u32));
    }

    writer.write_u32_le(payload.len() as u32).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}
