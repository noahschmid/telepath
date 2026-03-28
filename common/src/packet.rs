use crate::error::PacketError;

/// Header size in bytes: seq(4) + timestamp_us(8) + frame_count(2) = 14
pub const HEADER_SIZE: usize = 14;

/// Maximum UDP payload we'll ever send. 1024 f32 frames = 4096 bytes + 14 header
/// = 4110 bytes, well under the 8192 byte practical UDP ceiling on LAN.
pub const MAX_FRAMES: usize = 1024;
pub const MAX_PACKET_BYTES: usize = HEADER_SIZE + MAX_FRAMES * 4;

/// A decoded audio packet ready for the jitter buffer.
#[derive(Debug, Clone)]
pub struct AudioPacket {
    /// Monotonically increasing sequence number. Wraps at u32::MAX.
    pub seq: u32,
    /// Sender clock in microseconds at the moment of capture.
    /// Used for jitter measurement — not for clock sync.
    pub timestamp_us: u64,
    /// PCM samples, f32 normalised [-1.0, 1.0], mono.
    pub samples: Vec<f32>,
}

impl AudioPacket {
    /// Encode into a stack-allocated byte buffer.
    /// Returns the number of bytes written.
    pub fn encode_into(&self, buf: &mut [u8]) -> Result<usize, PacketError> {
        let frame_count = self.samples.len();
        if frame_count > MAX_FRAMES {
            return Err(PacketError::TooManyFrames(frame_count));
        }
        let total = HEADER_SIZE + frame_count * 4;
        if buf.len() < total {
            return Err(PacketError::BufferTooSmall { need: total, got: buf.len() });
        }

        buf[0..4].copy_from_slice(&self.seq.to_le_bytes());
        buf[4..12].copy_from_slice(&self.timestamp_us.to_le_bytes());
        buf[12..14].copy_from_slice(&(frame_count as u16).to_le_bytes());

        for (i, &s) in self.samples.iter().enumerate() {
            let offset = HEADER_SIZE + i * 4;
            buf[offset..offset + 4].copy_from_slice(&s.to_le_bytes());
        }

        Ok(total)
    }

    /// Decode from a raw UDP datagram.
    pub fn decode(buf: &[u8]) -> Result<Self, PacketError> {
        if buf.len() < HEADER_SIZE {
            return Err(PacketError::TooShort { got: buf.len() });
        }

        let seq = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let timestamp_us = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        let frame_count = u16::from_le_bytes(buf[12..14].try_into().unwrap()) as usize;

        let expected_len = HEADER_SIZE + frame_count * 4;
        if buf.len() < expected_len {
            return Err(PacketError::Truncated {
                expected: expected_len,
                got: buf.len(),
            });
        }

        let samples = (0..frame_count)
            .map(|i| {
                let offset = HEADER_SIZE + i * 4;
                f32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
            })
            .collect();

        Ok(Self { seq, timestamp_us, samples })
    }

    /// Convenience: encode into a freshly allocated Vec.
    pub fn encode(&self) -> Result<Vec<u8>, PacketError> {
        let mut buf = vec![0u8; HEADER_SIZE + self.samples.len() * 4];
        let n = self.encode_into(&mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
}

/// Returns the current time in microseconds from a monotonic clock.
/// Use this on the sender side to fill `timestamp_us`.
pub fn now_us() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
