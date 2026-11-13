// SPDX-License-Identifier: CC0-1.0

//! The RESP FRAMING of a cache's socket: the bytes a command arrives as, and the bytes a
//! reply goes back as.
//!
//! The format is the protocol's own, decoded and encoded by the `redis-protocol` crate
//! rather than by a second reading of RESP written here — a hand-rolled framer answers
//! every input it was not written for wrongly. This module is only the seam onto it: a
//! command as the list of arguments it is ([`next_command`]) and a reply as the bytes it
//! serialises to ([`encoded`]), so nothing above it names a frame type.
//!
//! # Bounds
//!
//! A client's buffer is capped at [`MAX_COMMAND_BYTES`]: a connection that sends more than
//! that without completing a command is answered with a protocol error and closed, so no
//! session can make the emulator buffer without end. The decoder itself refuses a frame
//! whose declared length is past what it has, so a declared length is never allocated.

use redis_protocol::resp2::decode::decode;
use redis_protocol::resp2::encode::encode;
use redis_protocol::resp2::types::{OwnedFrame, Resp2Frame};

/// The most bytes one connection may have buffered without completing a command.
///
/// A command is a handful of arguments and one value; this leaves room for the largest
/// value a cache's keyspace takes ([`super::keyspace::MAX_VALUE_BYTES`]) and its framing,
/// and nothing beyond it.
pub const MAX_COMMAND_BYTES: usize = super::keyspace::MAX_VALUE_BYTES + 64 * 1024;

/// What one reply is, in the terms the command layer answers in — mapped onto RESP frames
/// here so no command handler names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// A status reply (`+OK`).
    Status(String),
    /// An error reply (`-ERR …`), whose first word is the error's code.
    Error(String),
    /// An integer reply (`:1`).
    Integer(i64),
    /// A bulk reply: the bytes of one value.
    Bulk(Vec<u8>),
    /// The null bulk reply: a key that holds nothing.
    Nil,
    /// A multi-bulk reply, each element one of the above.
    Array(Vec<Reply>),
}

impl Reply {
    /// The `+OK` every command that merely succeeds answers with.
    pub fn ok() -> Self {
        Self::Status("OK".to_string())
    }

    /// A bulk reply carrying `text`.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Bulk(text.into().into_bytes())
    }

    /// This reply as the RESP frame it is.
    fn frame(self) -> OwnedFrame {
        match self {
            Self::Status(status) => OwnedFrame::SimpleString(status.into_bytes()),
            Self::Error(error) => OwnedFrame::Error(error),
            Self::Integer(value) => OwnedFrame::Integer(value),
            Self::Bulk(bytes) => OwnedFrame::BulkString(bytes),
            Self::Nil => OwnedFrame::Null,
            Self::Array(elements) => {
                OwnedFrame::Array(elements.into_iter().map(Reply::frame).collect())
            }
        }
    }
}

/// The bytes `reply` goes on the wire as.
///
/// An integer reply is encoded as RESP's own integer (`:1`) rather than as a bulk string,
/// which is what every command that answers a count answers with.
pub fn encoded(reply: Reply) -> Vec<u8> {
    let frame = reply.frame();
    let mut buffer = vec![0u8; frame.encode_len(false)];
    match encode(&mut buffer, &frame, false) {
        Ok(written) => {
            buffer.truncate(written);
            buffer
        }
        // Unreachable: the buffer is the length the frame itself reports. An encoder that
        // refuses it still has to answer the client something, and a protocol error is
        // the honest answer.
        Err(_) => b"-ERR this reply could not be encoded\r\n".to_vec(),
    }
}

/// One decoded command: the arguments it carries, and how many bytes of the connection's
/// buffer it consumed.
pub type DecodedCommand = (Vec<Vec<u8>>, usize);

/// Why a connection's bytes are not a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The bytes are not RESP at all, or not a command: the engine's own
    /// `ERR Protocol error`.
    Protocol(String),
    /// The buffer is past [`MAX_COMMAND_BYTES`] with no command completed.
    TooLong,
}

/// The next command in `buffer`, as its arguments, with how many bytes it consumed — or
/// `Ok(None)` when the buffer holds the beginning of one and the connection has to read
/// more.
///
/// Both forms a client sends are read: the RESP array every library sends, and the INLINE
/// form a line-oriented client (`redis-cli`'s own fallback, a hand-typed `PING`) sends,
/// which is a line of whitespace-separated words.
pub fn next_command(buffer: &[u8]) -> Result<Option<DecodedCommand>, FrameError> {
    if buffer.is_empty() {
        return Ok(None);
    }
    if buffer.len() > MAX_COMMAND_BYTES {
        return Err(FrameError::TooLong);
    }
    if buffer[0] != b'*' {
        return inline_command(buffer);
    }
    // The SHAPE is checked before the decoder walks the buffer: a decoder parses a nested
    // aggregate by recursive descent, and a client is under no obligation to send a
    // command the server wants, so a payload of nested arrays would recurse as deep as its
    // bytes say. A command is a FLAT multibulk by construction, so anything else is a
    // protocol error — decided iteratively, before anything is built.
    if !is_flat_multibulk(buffer) {
        return Err(FrameError::Protocol(
            "ERR Protocol error: expected '$'".to_string(),
        ));
    }
    match decode(buffer) {
        Ok(Some((frame, consumed))) => Ok(Some((command_arguments(frame)?, consumed))),
        Ok(None) => Ok(None),
        Err(error) => Err(FrameError::Protocol(format!(
            "ERR Protocol error: {}",
            error.details()
        ))),
    }
}

/// One `\r\n`-terminated line of `buffer`, and what follows it — `None` while the line is
/// incomplete.
fn next_line(buffer: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = buffer
        .windows(2)
        .position(|pair| pair == b"\r\n")?;
    Some((&buffer[..end], &buffer[end + 2..]))
}

/// The integer a `*` / `$` header line declares, or `None` when the line is not one.
fn declared_length(line: &[u8], marker: u8) -> Option<i64> {
    let digits = line.strip_prefix(&[marker])?;
    std::str::from_utf8(digits).ok()?.parse::<i64>().ok()
}

/// Whether `buffer` begins with the FLAT multibulk a command is — a `*<count>` header
/// followed by `$<length>` elements — rather than with a nested aggregate.
///
/// `true` for a buffer that is merely INCOMPLETE: the decoder answers `Ok(None)` for one
/// and the connection reads more, and the check runs again on the longer buffer. The walk
/// respects each element's declared length, so a VALUE carrying `\r\n*` is not mistaken
/// for a nested array.
fn is_flat_multibulk(buffer: &[u8]) -> bool {
    let Some((header, mut rest)) = next_line(buffer) else {
        return true;
    };
    let Some(count) = declared_length(header, b'*') else {
        // Not a count at all; the decoder's own protocol error says so.
        return true;
    };
    for _ in 0..count.max(0) {
        let Some((element, tail)) = next_line(rest) else {
            return true;
        };
        let Some(length) = declared_length(element, b'$') else {
            // An element that is not a bulk string: a nested aggregate, or a type a
            // command never carries.
            return false;
        };
        let skip = usize::try_from(length.max(0)).unwrap_or(0).saturating_add(2);
        if tail.len() < skip {
            return true;
        }
        rest = &tail[skip..];
    }
    true
}

/// The arguments of a decoded command frame: a RESP array of bulk strings, which is what a
/// client sends a command as.
fn command_arguments(frame: OwnedFrame) -> Result<Vec<Vec<u8>>, FrameError> {
    match frame {
        OwnedFrame::Array(elements) => elements
            .into_iter()
            .map(|element| match element {
                OwnedFrame::BulkString(bytes) | OwnedFrame::SimpleString(bytes) => Ok(bytes),
                OwnedFrame::Integer(value) => Ok(value.to_string().into_bytes()),
                _ => Err(FrameError::Protocol(
                    "ERR Protocol error: expected '$'".to_string(),
                )),
            })
            .collect(),
        _ => Err(FrameError::Protocol(
            "ERR Protocol error: expected '*'".to_string(),
        )),
    }
}

/// The INLINE form of a command: one line of whitespace-separated words. `Ok(None)` until
/// the line is complete.
fn inline_command(buffer: &[u8]) -> Result<Option<DecodedCommand>, FrameError> {
    let Some(end) = buffer.iter().position(|byte| *byte == b'\n') else {
        return Ok(None);
    };
    let line = &buffer[..end];
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let arguments: Vec<Vec<u8>> = line
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|word| !word.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    Ok(Some((arguments, end + 1)))
}
