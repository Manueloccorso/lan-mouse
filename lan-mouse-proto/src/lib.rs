use input_event::{Event as InputEvent, KeyboardEvent, PointerEvent};
use num_enum::{IntoPrimitive, TryFromPrimitive, TryFromPrimitiveError};
use paste::paste;
use std::{
    fmt::{Debug, Display, Formatter},
    mem::size_of,
};
use thiserror::Error;

/// Maximum size of a non-clipboard encoded event.
pub const MAX_INPUT_EVENT_SIZE: usize = size_of::<u8>() + size_of::<u32>() + 2 * size_of::<f64>();

/// Maximum clipboard text transferred in one transition.
pub const MAX_CLIPBOARD_SIZE: usize = 64 * 1024;
/// Clipboard chunks stay small enough to avoid IP fragmentation.
pub const CLIPBOARD_CHUNK_SIZE: usize = 1024;
/// Maximum serialized clipboard event size.
pub const CLIPBOARD_EVENT_SIZE: usize =
    size_of::<u8>() + size_of::<u32>() + size_of::<u16>() + size_of::<u16>() + CLIPBOARD_CHUNK_SIZE;
/// Maximum size of any encoded protocol event.
pub const MAX_EVENT_SIZE: usize = if MAX_INPUT_EVENT_SIZE > CLIPBOARD_EVENT_SIZE {
    MAX_INPUT_EVENT_SIZE
} else {
    CLIPBOARD_EVENT_SIZE
};
/// Alias describing the size used by protocol receive buffers.
pub const PROTOCOL_EVENT_SIZE: usize = MAX_EVENT_SIZE;

/// error type for protocol violations
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// event type does not exist
    #[error("invalid event id: `{0}`")]
    InvalidEventId(#[from] TryFromPrimitiveError<EventType>),
    /// position type does not exist
    #[error("invalid event id: `{0}`")]
    InvalidPosition(#[from] TryFromPrimitiveError<Position>),
    /// clipboard chunk contains an invalid length
    #[error("invalid clipboard chunk length: `{0}`")]
    InvalidClipboardChunkLength(usize),
}

/// Position of a client
#[derive(Clone, Copy, Debug, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum Position {
    Left,
    Right,
    Top,
    Bottom,
}

impl Display for Position {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let pos = match self {
            Position::Left => "left",
            Position::Right => "right",
            Position::Top => "top",
            Position::Bottom => "bottom",
        };
        write!(f, "{pos}")
    }
}

/// main lan-mouse protocol event type
#[derive(Clone, Copy, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ProtoEvent {
    /// notify a client that the cursor entered its region at the given position
    /// [`ProtoEvent::Ack`] with the same serial is used for synchronization between devices
    Enter(Position),
    /// notify a client that the cursor left its region
    /// [`ProtoEvent::Ack`] with the same serial is used for synchronization between devices
    Leave(u32),
    /// acknowledge of an [`ProtoEvent::Enter`] or [`ProtoEvent::Leave`] event
    Ack(u32),
    /// Input event
    Input(InputEvent),
    /// Ping event for tracking unresponsive clients.
    /// A client has to respond with [`ProtoEvent::Pong`].
    Ping,
    /// Response to [`ProtoEvent::Ping`], true if emulation is enabled / available
    Pong(bool),
    /// Build identification for the sending peer. Sent by the
    /// connect side once after the connection authenticates, and
    /// echoed back by the listen side in reply, so each end can
    /// display the peer's build hash and warn (soft) on mismatch.
    /// `commit` is the 8-byte ASCII short commit hash from
    /// `shadow_rs`'s `SHORT_COMMIT`. Old peers that don't
    /// recognize the event type silently skip it per the
    /// forward-compat handling in the receive loop.
    Hello { commit: [u8; 8] },
    /// Start of a bounded clipboard transfer.
    ClipboardStart { transfer_id: u32, total_bytes: u32 },
    /// A chunk of a clipboard transfer.
    ClipboardChunk {
        transfer_id: u32,
        chunk: u16,
        total_chunks: u16,
        len: u16,
        data: [u8; CLIPBOARD_CHUNK_SIZE],
    },
    /// Acknowledge a completed clipboard transfer.
    ClipboardAck { transfer_id: u32 },
}

impl Display for ProtoEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoEvent::Enter(s) => write!(f, "Enter({s})"),
            ProtoEvent::Leave(s) => write!(f, "Leave({s})"),
            ProtoEvent::Ack(s) => write!(f, "Ack({s})"),
            ProtoEvent::Input(e) => write!(f, "{e}"),
            ProtoEvent::Ping => write!(f, "ping"),
            ProtoEvent::Pong(alive) => {
                write!(
                    f,
                    "pong: {}",
                    if *alive { "alive" } else { "not available" }
                )
            }
            ProtoEvent::Hello { commit } => {
                let s = std::str::from_utf8(commit).unwrap_or("????????");
                write!(f, "Hello({s})")
            }
            ProtoEvent::ClipboardStart {
                transfer_id,
                total_bytes,
            } => write!(f, "ClipboardStart({transfer_id}, {total_bytes} bytes)"),
            ProtoEvent::ClipboardChunk {
                transfer_id,
                chunk,
                total_chunks,
                len,
                ..
            } => write!(
                f,
                "ClipboardChunk({transfer_id}, {chunk}/{total_chunks}, {len} bytes)"
            ),
            ProtoEvent::ClipboardAck { transfer_id } => write!(f, "ClipboardAck({transfer_id})"),
        }
    }
}

#[derive(TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum EventType {
    PointerMotion,
    PointerButton,
    PointerAxis,
    PointerAxisValue120,
    KeyboardKey,
    KeyboardModifiers,
    Ping,
    Pong,
    Enter,
    Leave,
    Ack,
    Hello,
    ClipboardStart,
    ClipboardChunk,
    ClipboardAck,
}

impl ProtoEvent {
    fn event_type(&self) -> EventType {
        match self {
            ProtoEvent::Input(e) => match e {
                InputEvent::Pointer(p) => match p {
                    PointerEvent::Motion { .. } => EventType::PointerMotion,
                    PointerEvent::Button { .. } => EventType::PointerButton,
                    PointerEvent::Axis { .. } => EventType::PointerAxis,
                    PointerEvent::AxisDiscrete120 { .. } => EventType::PointerAxisValue120,
                },
                InputEvent::Keyboard(k) => match k {
                    KeyboardEvent::Key { .. } => EventType::KeyboardKey,
                    KeyboardEvent::Modifiers { .. } => EventType::KeyboardModifiers,
                },
            },
            ProtoEvent::Ping => EventType::Ping,
            ProtoEvent::Pong(_) => EventType::Pong,
            ProtoEvent::Enter(_) => EventType::Enter,
            ProtoEvent::Leave(_) => EventType::Leave,
            ProtoEvent::Ack(_) => EventType::Ack,
            ProtoEvent::Hello { .. } => EventType::Hello,
            ProtoEvent::ClipboardStart { .. } => EventType::ClipboardStart,
            ProtoEvent::ClipboardChunk { .. } => EventType::ClipboardChunk,
            ProtoEvent::ClipboardAck { .. } => EventType::ClipboardAck,
        }
    }
}

impl TryFrom<[u8; PROTOCOL_EVENT_SIZE]> for ProtoEvent {
    type Error = ProtocolError;

    fn try_from(buf: [u8; PROTOCOL_EVENT_SIZE]) -> Result<Self, Self::Error> {
        let mut buf = &buf[..];
        let event_type = decode_u8(&mut buf)?;
        match EventType::try_from(event_type)? {
            EventType::PointerMotion => {
                Ok(Self::Input(InputEvent::Pointer(PointerEvent::Motion {
                    time: decode_u32(&mut buf)?,
                    dx: decode_f64(&mut buf)?,
                    dy: decode_f64(&mut buf)?,
                })))
            }
            EventType::PointerButton => {
                Ok(Self::Input(InputEvent::Pointer(PointerEvent::Button {
                    time: decode_u32(&mut buf)?,
                    button: decode_u32(&mut buf)?,
                    state: decode_u32(&mut buf)?,
                })))
            }
            EventType::PointerAxis => Ok(Self::Input(InputEvent::Pointer(PointerEvent::Axis {
                time: decode_u32(&mut buf)?,
                axis: decode_u8(&mut buf)?,
                value: decode_f64(&mut buf)?,
            }))),
            EventType::PointerAxisValue120 => Ok(Self::Input(InputEvent::Pointer(
                PointerEvent::AxisDiscrete120 {
                    axis: decode_u8(&mut buf)?,
                    value: decode_i32(&mut buf)?,
                },
            ))),
            EventType::KeyboardKey => Ok(Self::Input(InputEvent::Keyboard(KeyboardEvent::Key {
                time: decode_u32(&mut buf)?,
                key: decode_u32(&mut buf)?,
                state: decode_u8(&mut buf)?,
            }))),
            EventType::KeyboardModifiers => Ok(Self::Input(InputEvent::Keyboard(
                KeyboardEvent::Modifiers {
                    depressed: decode_u32(&mut buf)?,
                    latched: decode_u32(&mut buf)?,
                    locked: decode_u32(&mut buf)?,
                    group: decode_u32(&mut buf)?,
                },
            ))),
            EventType::Ping => Ok(Self::Ping),
            EventType::Pong => Ok(Self::Pong(decode_u8(&mut buf)? != 0)),
            EventType::Enter => Ok(Self::Enter(decode_u8(&mut buf)?.try_into()?)),
            EventType::Leave => Ok(Self::Leave(decode_u32(&mut buf)?)),
            EventType::Ack => Ok(Self::Ack(decode_u32(&mut buf)?)),
            EventType::Hello => {
                let mut commit = [0u8; 8];
                for b in commit.iter_mut() {
                    *b = decode_u8(&mut buf)?;
                }
                Ok(Self::Hello { commit })
            }
            EventType::ClipboardStart => Ok(Self::ClipboardStart {
                transfer_id: decode_u32(&mut buf)?,
                total_bytes: decode_u32(&mut buf)?,
            }),
            EventType::ClipboardChunk => {
                let transfer_id = decode_u32(&mut buf)?;
                let chunk = decode_u16(&mut buf)?;
                let total_chunks = decode_u16(&mut buf)?;
                let len = decode_u16(&mut buf)?;
                let len = usize::from(len);
                if len > CLIPBOARD_CHUNK_SIZE {
                    return Err(ProtocolError::InvalidClipboardChunkLength(len));
                }
                let mut data = [0; CLIPBOARD_CHUNK_SIZE];
                data[..len].copy_from_slice(&buf[..len]);
                Ok(Self::ClipboardChunk {
                    transfer_id,
                    chunk,
                    total_chunks,
                    len: len as u16,
                    data,
                })
            }
            EventType::ClipboardAck => Ok(Self::ClipboardAck {
                transfer_id: decode_u32(&mut buf)?,
            }),
        }
    }
}

impl From<ProtoEvent> for ([u8; PROTOCOL_EVENT_SIZE], usize) {
    fn from(event: ProtoEvent) -> Self {
        let mut buf = [0u8; PROTOCOL_EVENT_SIZE];
        let mut len = 0usize;
        {
            let mut buf = &mut buf[..];
            let buf = &mut buf;
            let len = &mut len;
            encode_u8(buf, len, event.event_type() as u8);
            match event {
                ProtoEvent::Input(event) => match event {
                    InputEvent::Pointer(p) => match p {
                        PointerEvent::Motion { time, dx, dy } => {
                            encode_u32(buf, len, time);
                            encode_f64(buf, len, dx);
                            encode_f64(buf, len, dy);
                        }
                        PointerEvent::Button {
                            time,
                            button,
                            state,
                        } => {
                            encode_u32(buf, len, time);
                            encode_u32(buf, len, button);
                            encode_u32(buf, len, state);
                        }
                        PointerEvent::Axis { time, axis, value } => {
                            encode_u32(buf, len, time);
                            encode_u8(buf, len, axis);
                            encode_f64(buf, len, value);
                        }
                        PointerEvent::AxisDiscrete120 { axis, value } => {
                            encode_u8(buf, len, axis);
                            encode_i32(buf, len, value);
                        }
                    },
                    InputEvent::Keyboard(k) => match k {
                        KeyboardEvent::Key { time, key, state } => {
                            encode_u32(buf, len, time);
                            encode_u32(buf, len, key);
                            encode_u8(buf, len, state);
                        }
                        KeyboardEvent::Modifiers {
                            depressed,
                            latched,
                            locked,
                            group,
                        } => {
                            encode_u32(buf, len, depressed);
                            encode_u32(buf, len, latched);
                            encode_u32(buf, len, locked);
                            encode_u32(buf, len, group);
                        }
                    },
                },
                ProtoEvent::Ping => {}
                ProtoEvent::Pong(alive) => encode_u8(buf, len, alive as u8),
                ProtoEvent::Enter(pos) => encode_u8(buf, len, pos as u8),
                ProtoEvent::Leave(serial) => encode_u32(buf, len, serial),
                ProtoEvent::Ack(serial) => encode_u32(buf, len, serial),
                ProtoEvent::Hello { commit } => {
                    for b in commit.iter() {
                        encode_u8(buf, len, *b);
                    }
                }
                ProtoEvent::ClipboardStart {
                    transfer_id,
                    total_bytes,
                } => {
                    encode_u32(buf, len, transfer_id);
                    encode_u32(buf, len, total_bytes);
                }
                ProtoEvent::ClipboardChunk {
                    transfer_id,
                    chunk,
                    total_chunks,
                    len: data_len,
                    data,
                } => {
                    let data_len = usize::from(data_len).min(CLIPBOARD_CHUNK_SIZE);
                    encode_u32(buf, len, transfer_id);
                    encode_u16(buf, len, chunk);
                    encode_u16(buf, len, total_chunks);
                    encode_u16(buf, len, data_len as u16);
                    for byte in &data[..data_len] {
                        encode_u8(buf, len, *byte);
                    }
                }
                ProtoEvent::ClipboardAck { transfer_id } => encode_u32(buf, len, transfer_id),
            }
        }
        (buf, len)
    }
}

macro_rules! decode_impl {
    ($t:ty) => {
        paste! {
            fn [<decode_ $t>](data: &mut &[u8]) -> Result<$t, ProtocolError> {
                let (int_bytes, rest) = data.split_at(size_of::<$t>());
                *data = rest;
                Ok($t::from_be_bytes(int_bytes.try_into().unwrap()))
            }
        }
    };
}

decode_impl!(u8);
decode_impl!(u32);
decode_impl!(u16);
decode_impl!(i32);
decode_impl!(f64);

macro_rules! encode_impl {
    ($t:ty) => {
        paste! {
            fn [<encode_ $t>](buf: &mut &mut [u8], amt: &mut usize, n: $t) {
                let src = n.to_be_bytes();
                let data = std::mem::take(buf);
                let (int_bytes, rest) = data.split_at_mut(size_of::<$t>());
                int_bytes.copy_from_slice(&src);
                *amt += size_of::<$t>();
                *buf = rest
            }
        }
    };
}

encode_impl!(u8);
encode_impl!(u32);
encode_impl!(u16);
encode_impl!(i32);
encode_impl!(f64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_chunk_round_trip() {
        let mut data = [0; CLIPBOARD_CHUNK_SIZE];
        data[..3].copy_from_slice(b"abc");
        let event = ProtoEvent::ClipboardChunk {
            transfer_id: 1,
            chunk: 0,
            total_chunks: 1,
            len: 3,
            data,
        };
        let (encoded, len): ([u8; PROTOCOL_EVENT_SIZE], usize) = event.into();
        assert_eq!(len, 1 + 4 + 2 + 2 + 2 + 3);
        let decoded = ProtoEvent::try_from(encoded).expect("decode clipboard chunk");
        assert!(matches!(decoded, ProtoEvent::ClipboardChunk { len: 3, .. }));
    }
}
