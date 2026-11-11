use std::fmt;

pub const MAX_FRAME_PAYLOAD: usize = 1024 * 1024;
const FRAME_PREFIX_LEN: usize = 4;
const FRAME_HEADER_LEN: usize = FRAME_PREFIX_LEN + 1;
const OUTPUT_HEADER_LEN: usize = 16 + 8;

/// The `seq` of a kind-1 frame that carries a first paint rather than stream
/// bytes: it is not an offset, and the receiver parses it without moving its
/// own offset.
pub const PAINT_SEQ: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 0,
    TerminalOutput = 1,
}

impl TryFrom<u8> for FrameKind {
    type Error = FrameError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Control),
            1 => Ok(Self::TerminalOutput),
            _ => Err(FrameError::Malformed("unknown frame kind")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    Incomplete,
    FrameTooLarge(usize),
    Malformed(&'static str),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete => f.write_str("incomplete frame"),
            Self::FrameTooLarge(length) => write!(f, "frame payload too large: {length} bytes"),
            Self::Malformed(reason) => write!(f, "malformed frame: {reason}"),
        }
    }
}

impl std::error::Error for FrameError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFrame<'a> {
    pub session_id: [u8; 16],
    pub seq: u64,
    pub data: &'a [u8],
}

pub fn encode(frame: &Frame) -> Result<Vec<u8>, FrameError> {
    if frame.payload.len() > MAX_FRAME_PAYLOAD {
        return Err(FrameError::FrameTooLarge(frame.payload.len()));
    }

    let payload_len = u32::try_from(frame.payload.len())
        .map_err(|_| FrameError::FrameTooLarge(frame.payload.len()))?;
    let mut encoded = Vec::with_capacity(FRAME_HEADER_LEN + frame.payload.len());
    encoded.extend_from_slice(&payload_len.to_be_bytes());
    encoded.push(frame.kind as u8);
    encoded.extend_from_slice(&frame.payload);
    Ok(encoded)
}

pub fn decode(input: &[u8]) -> Result<(Frame, usize), FrameError> {
    if input.len() < FRAME_PREFIX_LEN {
        return Err(FrameError::Incomplete);
    }

    let payload_len = u32::from_be_bytes(input[..FRAME_PREFIX_LEN].try_into().unwrap()) as usize;
    if payload_len > MAX_FRAME_PAYLOAD {
        return Err(FrameError::FrameTooLarge(payload_len));
    }
    if input.len() < FRAME_HEADER_LEN {
        return Err(FrameError::Incomplete);
    }

    let kind = FrameKind::try_from(input[FRAME_PREFIX_LEN])?;
    let frame_len = FRAME_HEADER_LEN + payload_len;
    if input.len() < frame_len {
        return Err(FrameError::Incomplete);
    }

    Ok((
        Frame {
            kind,
            payload: input[FRAME_HEADER_LEN..frame_len].to_vec(),
        },
        frame_len,
    ))
}

pub fn encode_output(session_id: [u8; 16], seq: u64, data: &[u8]) -> Result<Vec<u8>, FrameError> {
    if data.len() > MAX_FRAME_PAYLOAD - OUTPUT_HEADER_LEN {
        return Err(FrameError::FrameTooLarge(OUTPUT_HEADER_LEN + data.len()));
    }

    let mut payload = Vec::with_capacity(OUTPUT_HEADER_LEN + data.len());
    payload.extend_from_slice(&session_id);
    payload.extend_from_slice(&seq.to_be_bytes());
    payload.extend_from_slice(data);
    Ok(payload)
}

pub fn decode_output(payload: &[u8]) -> Result<OutputFrame<'_>, FrameError> {
    if payload.len() < OUTPUT_HEADER_LEN {
        return Err(FrameError::Malformed(
            "terminal output payload is shorter than 24 bytes",
        ));
    }

    Ok(OutputFrame {
        session_id: payload[..16].try_into().unwrap(),
        seq: u64::from_be_bytes(payload[16..OUTPUT_HEADER_LEN].try_into().unwrap()),
        data: &payload[OUTPUT_HEADER_LEN..],
    })
}

#[cfg(test)]
mod tests {
    use super::{decode, decode_output, encode, encode_output, Frame, FrameError, FrameKind};

    #[test]
    fn control_frame_round_trips_json_payload() {
        let payload = br#"{"type":"system.health","id":1}"#;
        let encoded = encode(&Frame {
            kind: FrameKind::Control,
            payload: payload.to_vec(),
        })
        .unwrap();

        let (decoded, consumed) = decode(&encoded).unwrap();

        assert_eq!(decoded.kind, FrameKind::Control);
        assert_eq!(decoded.payload, payload);
        assert_eq!(consumed, encoded.len());
    }

    #[test]
    fn output_payload_round_trips() {
        let session_id = [7; 16];
        let payload = encode_output(session_id, 42, b"hello").unwrap();
        let encoded = encode(&Frame {
            kind: FrameKind::TerminalOutput,
            payload,
        })
        .unwrap();
        let (frame, consumed) = decode(&encoded).unwrap();
        let decoded = decode_output(&frame.payload).unwrap();

        assert_eq!(frame.kind, FrameKind::TerminalOutput);
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.session_id, session_id);
        assert_eq!(decoded.seq, 42);
        assert_eq!(decoded.data, b"hello");
    }

    #[test]
    fn oversized_length_is_rejected_before_payload_is_present() {
        let too_large = (1024_u32 * 1024 + 1).to_be_bytes();

        assert_eq!(
            decode(&too_large),
            Err(FrameError::FrameTooLarge(1024 * 1024 + 1))
        );
    }

    #[test]
    fn incomplete_frame_is_reported_without_panicking() {
        assert_eq!(decode(&[0, 0, 0]), Err(FrameError::Incomplete));
        assert_eq!(decode(&[0, 0, 0, 2, 0, b'{']), Err(FrameError::Incomplete));
    }

    #[test]
    fn unknown_kind_is_malformed() {
        assert_eq!(
            decode(&[0, 0, 0, 0, 9]),
            Err(FrameError::Malformed("unknown frame kind"))
        );
    }

    #[test]
    fn short_output_payload_is_malformed() {
        assert_eq!(
            decode_output(&[0; 23]),
            Err(FrameError::Malformed(
                "terminal output payload is shorter than 24 bytes"
            ))
        );
    }
}
