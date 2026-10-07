//! GbxRemote 2 framing.
//!
//! After the handshake, every message in both directions is
//! `[u32 LE payload size][u32 LE handle][payload]`. Handles allocated by the client start at
//! `0x8000_0000`; the server answers with the same handle. Messages initiated by the server
//! (callbacks) carry a handle below `0x8000_0000`.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::codec::{Decoder, Encoder};

use crate::error::{FrameError, GbxError};

/// Magic string sent by the server right after the TCP connection is opened.
pub const PROTOCOL_HEADER: &str = "GBXRemote 2";

/// First handle of the client range; handles at or above this value are client requests.
pub const CLIENT_HANDLE_START: u32 = 0x8000_0000;

/// Header length (size + handle).
const HEADER_LEN: usize = 8;

/// One framed message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub handle: u32,
    pub payload: Bytes,
}

impl Frame {
    /// `true` when the frame was initiated by the server (a callback).
    pub fn is_callback(&self) -> bool {
        self.handle < CLIENT_HANDLE_START
    }
}

/// Encoder/decoder for [`Frame`]s with a maximum payload size in each direction.
#[derive(Debug, Clone)]
pub struct GbxCodec {
    max_incoming: usize,
    max_outgoing: usize,
}

impl GbxCodec {
    pub fn new(max_incoming: usize, max_outgoing: usize) -> Self {
        Self {
            max_incoming,
            max_outgoing,
        }
    }
}

impl Default for GbxCodec {
    /// 32 MiB incoming (map lists can be large), 4 MiB outgoing: the most conservative limit used by
    /// existing clients for TM2020 requests.
    fn default() -> Self {
        Self::new(32 * 1024 * 1024, 4 * 1024 * 1024)
    }
}

impl Decoder for GbxCodec {
    type Item = Frame;
    type Error = FrameError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, FrameError> {
        if src.len() < HEADER_LEN {
            return Ok(None);
        }
        let size = u32::from_le_bytes([src[0], src[1], src[2], src[3]]) as usize;
        if size > self.max_incoming {
            return Err(FrameError::TooLarge {
                size,
                max: self.max_incoming,
            });
        }
        if src.len() < HEADER_LEN + size {
            src.reserve(HEADER_LEN + size - src.len());
            return Ok(None);
        }
        src.advance(4);
        let handle = src.get_u32_le();
        let payload = src.split_to(size).freeze();
        Ok(Some(Frame { handle, payload }))
    }
}

impl Encoder<Frame> for GbxCodec {
    type Error = FrameError;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> Result<(), FrameError> {
        let size = frame.payload.len();
        if size > self.max_outgoing {
            return Err(FrameError::TooLarge {
                size,
                max: self.max_outgoing,
            });
        }
        let size_u32 = u32::try_from(size).map_err(|_| FrameError::TooLarge {
            size,
            max: self.max_outgoing,
        })?;
        dst.reserve(HEADER_LEN + size);
        dst.put_u32_le(size_u32);
        dst.put_u32_le(frame.handle);
        dst.extend_from_slice(&frame.payload);
        Ok(())
    }
}

/// Reads and validates the protocol header sent by the server on connection.
pub async fn read_handshake<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(), GbxError> {
    let size = reader.read_u32_le().await.map_err(FrameError::Io)? as usize;
    // The header is tiny; refuse anything absurd rather than allocating it.
    if size == 0 || size > 64 {
        return Err(GbxError::Handshake(format!("invalid header length {size}")));
    }
    let mut header = vec![0u8; size];
    reader
        .read_exact(&mut header)
        .await
        .map_err(FrameError::Io)?;
    let header = String::from_utf8_lossy(&header);
    if header != PROTOCOL_HEADER {
        return Err(GbxError::Handshake(format!(
            "unsupported protocol {header:?}, expected {PROTOCOL_HEADER:?}"
        )));
    }
    Ok(())
}

/// Allocates request handles in `[0x8000_0000, 0xFFFF_FFFF]`, wrapping around.
#[derive(Debug)]
pub struct HandleAllocator {
    next: u32,
}

impl Default for HandleAllocator {
    fn default() -> Self {
        Self {
            next: CLIENT_HANDLE_START,
        }
    }
}

impl HandleAllocator {
    pub fn next_handle(&mut self) -> u32 {
        let handle = self.next;
        self.next = if handle == u32::MAX {
            CLIENT_HANDLE_START
        } else {
            handle + 1
        };
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_frame(handle: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(&handle.to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn decodes_a_response_frame() {
        let mut buf = BytesMut::from(&raw_frame(0x8000_0001, b"<methodResponse/>")[..]);
        let frame = GbxCodec::default().decode(&mut buf).unwrap().unwrap();
        assert_eq!(frame.handle, 0x8000_0001);
        assert!(!frame.is_callback());
        assert_eq!(&frame.payload[..], b"<methodResponse/>");
        assert!(buf.is_empty());
    }

    #[test]
    fn callback_frames_have_low_handles() {
        let mut buf = BytesMut::from(&raw_frame(0x0000_0042, b"<methodCall/>")[..]);
        let frame = GbxCodec::default().decode(&mut buf).unwrap().unwrap();
        assert!(frame.is_callback());
    }

    #[test]
    fn waits_for_partial_frames_and_splits_coalesced_ones() {
        let mut codec = GbxCodec::default();
        let mut bytes = raw_frame(0x8000_0000, b"first");
        bytes.extend(raw_frame(7, b"second"));

        let mut buf = BytesMut::new();
        buf.extend_from_slice(&bytes[..5]);
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        buf.extend_from_slice(&bytes[5..12]);
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        buf.extend_from_slice(&bytes[12..]);

        let first = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(&first.payload[..], b"first");
        let second = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!((second.handle, &second.payload[..]), (7, &b"second"[..]));
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
    }

    #[test]
    fn rejects_oversized_frames() {
        let mut codec = GbxCodec::new(4, 4);
        let mut buf = BytesMut::from(&raw_frame(0x8000_0000, b"too long")[..]);
        assert!(matches!(
            codec.decode(&mut buf),
            Err(FrameError::TooLarge { size: 8, max: 4 })
        ));
        let mut out = BytesMut::new();
        let frame = Frame {
            handle: 0x8000_0000,
            payload: Bytes::from_static(b"too long"),
        };
        assert!(codec.encode(frame, &mut out).is_err());
    }

    #[test]
    fn encodes_little_endian_header() {
        let mut out = BytesMut::new();
        let frame = Frame {
            handle: 0x8000_0002,
            payload: Bytes::from_static(b"abc"),
        };
        GbxCodec::default().encode(frame, &mut out).unwrap();
        assert_eq!(&out[..], &[3, 0, 0, 0, 0x02, 0, 0, 0x80, b'a', b'b', b'c']);
    }

    #[tokio::test]
    async fn accepts_the_gbxremote2_header() {
        let mut data: &[u8] = &[
            11, 0, 0, 0, b'G', b'B', b'X', b'R', b'e', b'm', b'o', b't', b'e', b' ', b'2',
        ];
        read_handshake(&mut data).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_the_gbxremote1_header() {
        let mut data: &[u8] = &[
            11, 0, 0, 0, b'G', b'B', b'X', b'R', b'e', b'm', b'o', b't', b'e', b' ', b'1',
        ];
        assert!(matches!(
            read_handshake(&mut data).await,
            Err(GbxError::Handshake(_))
        ));
    }

    #[test]
    fn handles_start_at_high_bit_and_wrap() {
        let mut alloc = HandleAllocator::default();
        assert_eq!(alloc.next_handle(), 0x8000_0000);
        assert_eq!(alloc.next_handle(), 0x8000_0001);
        alloc.next = u32::MAX;
        assert_eq!(alloc.next_handle(), u32::MAX);
        assert_eq!(alloc.next_handle(), 0x8000_0000);
    }
}
