/// Hand-rolled binary codec for `ClientMsg` and `ServerMsg`.
///
/// Wire format: 1-byte discriminant tag, fields in declaration order.
/// Integers are little-endian. Strings are 2-byte LE length then UTF-8 bytes.
///
/// This module is intentionally self-contained and zero-dependency.
use super::{
    AudioFrame, ClientMsg, GpioState, LatencyStats, RadioEvent, ServerMsg, StreamFlags,
    FRAME_BYTES, SAMPLES_PER_FRAME,
};

// ── Helper readers / writers ─────────────────────────────────────────────────

fn push_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}
fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_str(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    push_u16(buf, bytes.len() as u16);
    buf.extend_from_slice(bytes);
}

fn read_u8(buf: &[u8]) -> Option<(u8, &[u8])> {
    buf.split_first().map(|(&b, rest)| (b, rest))
}
fn read_u16(buf: &[u8]) -> Option<(u16, &[u8])> {
    if buf.len() < 2 { return None; }
    Some((u16::from_le_bytes(buf[..2].try_into().unwrap()), &buf[2..]))
}
fn read_u32(buf: &[u8]) -> Option<(u32, &[u8])> {
    if buf.len() < 4 { return None; }
    Some((u32::from_le_bytes(buf[..4].try_into().unwrap()), &buf[4..]))
}
fn read_u64(buf: &[u8]) -> Option<(u64, &[u8])> {
    if buf.len() < 8 { return None; }
    Some((u64::from_le_bytes(buf[..8].try_into().unwrap()), &buf[8..]))
}
fn read_str(buf: &[u8]) -> Option<(String, &[u8])> {
    let (len, rest) = read_u16(buf)?;
    let len = len as usize;
    if rest.len() < len { return None; }
    let s = std::str::from_utf8(&rest[..len]).ok()?.to_string();
    Some((s, &rest[len..]))
}

// ── Encode trait ──────────────────────────────────────────────────────────────

pub trait Encode {
    fn encode(&self, buf: &mut Vec<u8>);

    fn to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.encode(&mut buf);
        buf
    }
}

// ── Decode trait ──────────────────────────────────────────────────────────────

pub trait Decode: Sized {
    /// Decode from `buf`, returning the value and the remaining unconsumed bytes.
    fn decode(buf: &[u8]) -> Option<(Self, &[u8])>;

    fn from_bytes(buf: &[u8]) -> Option<Self> {
        let (val, _) = Self::decode(buf)?;
        Some(val)
    }
}

// ── StreamFlags ───────────────────────────────────────────────────────────────

impl Encode for StreamFlags {
    fn encode(&self, buf: &mut Vec<u8>) { push_u8(buf, self.0); }
}
impl Decode for StreamFlags {
    fn decode(buf: &[u8]) -> Option<(Self, &[u8])> {
        let (b, rest) = read_u8(buf)?;
        Some((StreamFlags::from_bits_truncate(b), rest))
    }
}

// ── GpioState ─────────────────────────────────────────────────────────────────

impl Encode for GpioState {
    fn encode(&self, buf: &mut Vec<u8>) { push_u8(buf, self.0); }
}
impl Decode for GpioState {
    fn decode(buf: &[u8]) -> Option<(Self, &[u8])> {
        let (b, rest) = read_u8(buf)?;
        Some((GpioState(b), rest))
    }
}

// ── LatencyStats ──────────────────────────────────────────────────────────────

impl Encode for LatencyStats {
    fn encode(&self, buf: &mut Vec<u8>) {
        push_u32(buf, self.min_us);
        push_u32(buf, self.max_us);
        push_u32(buf, self.p99_us);
    }
}
impl Decode for LatencyStats {
    fn decode(buf: &[u8]) -> Option<(Self, &[u8])> {
        let (min_us, buf) = read_u32(buf)?;
        let (max_us, buf) = read_u32(buf)?;
        let (p99_us, buf) = read_u32(buf)?;
        Some((LatencyStats { min_us, max_us, p99_us }, buf))
    }
}

// ── RadioEvent ────────────────────────────────────────────────────────────────
// Tag: 0=PttAssert 1=PttDeassert 2=CosActive 3=CosInactive 4=GpioChange(byte)

impl Encode for RadioEvent {
    fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            RadioEvent::PttAssert         => push_u8(buf, 0),
            RadioEvent::PttDeassert       => push_u8(buf, 1),
            RadioEvent::CosActive         => push_u8(buf, 2),
            RadioEvent::CosInactive       => push_u8(buf, 3),
            RadioEvent::GpioChange(g)     => { push_u8(buf, 4); g.encode(buf); }
        }
    }
}
impl Decode for RadioEvent {
    fn decode(buf: &[u8]) -> Option<(Self, &[u8])> {
        let (tag, rest) = read_u8(buf)?;
        match tag {
            0 => Some((RadioEvent::PttAssert,   rest)),
            1 => Some((RadioEvent::PttDeassert, rest)),
            2 => Some((RadioEvent::CosActive,   rest)),
            3 => Some((RadioEvent::CosInactive, rest)),
            4 => { let (g, rest) = GpioState::decode(rest)?; Some((RadioEvent::GpioChange(g), rest)) }
            _ => None,
        }
    }
}

// ── AudioFrame ────────────────────────────────────────────────────────────────

impl Encode for AudioFrame {
    fn encode(&self, buf: &mut Vec<u8>) {
        // Each i16 sample as 2 LE bytes.
        buf.reserve(FRAME_BYTES);
        for &s in &self.0 {
            buf.push((s & 0xff) as u8);
            buf.push(((s >> 8) & 0xff) as u8);
        }
    }
}
impl Decode for AudioFrame {
    fn decode(buf: &[u8]) -> Option<(Self, &[u8])> {
        if buf.len() < FRAME_BYTES { return None; }
        let mut frame = AudioFrame::default();
        for (i, sample) in frame.0.iter_mut().enumerate() {
            *sample = buf[i*2] as i16 | ((buf[i*2+1] as i16) << 8);
        }
        Some((frame, &buf[FRAME_BYTES..]))
    }
}

// ── ClientMsg ─────────────────────────────────────────────────────────────────
// Tag: 0=Subscribe 1=SetGpio 2=AudioWrite 3=Ping 4=GetStats

impl Encode for ClientMsg {
    fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            ClientMsg::Subscribe { streams } => {
                push_u8(buf, 0);
                streams.encode(buf);
            }
            ClientMsg::SetGpio { pin, high } => {
                push_u8(buf, 1);
                push_u8(buf, *pin);
                push_u8(buf, *high as u8);
            }
            ClientMsg::AudioWrite { frames } => {
                push_u8(buf, 2);
                push_u32(buf, frames.len() as u32);
                for frame in frames {
                    frame.encode(buf);
                }
            }
            ClientMsg::Ping    => push_u8(buf, 3),
            ClientMsg::GetStats => push_u8(buf, 4),
        }
    }
}
impl Decode for ClientMsg {
    fn decode(buf: &[u8]) -> Option<(Self, &[u8])> {
        let (tag, rest) = read_u8(buf)?;
        match tag {
            0 => {
                let (streams, rest) = StreamFlags::decode(rest)?;
                Some((ClientMsg::Subscribe { streams }, rest))
            }
            1 => {
                let (pin,  rest) = read_u8(rest)?;
                let (high, rest) = read_u8(rest)?;
                Some((ClientMsg::SetGpio { pin, high: high != 0 }, rest))
            }
            2 => {
                let (frame_count, mut rest) = read_u32(rest)?;
                // Sanity cap: 65 536 frames = ~64 s of audio. Anything beyond
                // that is almost certainly a corrupt length.
                if frame_count > 65_536 { return None; }
                let mut frames = Vec::with_capacity(frame_count as usize);
                for _ in 0..frame_count {
                    let (frame, r) = AudioFrame::decode(rest)?;
                    frames.push(frame);
                    rest = r;
                }
                Some((ClientMsg::AudioWrite { frames }, rest))
            }
            3 => Some((ClientMsg::Ping,     rest)),
            4 => Some((ClientMsg::GetStats, rest)),
            _ => None,
        }
    }
}

// ── ServerMsg ─────────────────────────────────────────────────────────────────
// Tag: 0=AudioReady 1=RadioEvent 2=Stats 3=Pong 4=Error

impl Encode for ServerMsg {
    fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            ServerMsg::AudioReady { seq } => {
                push_u8(buf, 0);
                push_u64(buf, *seq);
            }
            ServerMsg::RadioEvent(ev) => {
                push_u8(buf, 1);
                ev.encode(buf);
            }
            ServerMsg::Stats { rx_xruns, tx_xruns, dispatch_lat } => {
                push_u8(buf, 2);
                push_u64(buf, *rx_xruns);
                push_u64(buf, *tx_xruns);
                dispatch_lat.encode(buf);
            }
            ServerMsg::Pong    => push_u8(buf, 3),
            ServerMsg::Error(s) => {
                push_u8(buf, 4);
                push_str(buf, s);
            }
        }
    }
}
impl Decode for ServerMsg {
    fn decode(buf: &[u8]) -> Option<(Self, &[u8])> {
        let (tag, rest) = read_u8(buf)?;
        match tag {
            0 => {
                let (seq, rest) = read_u64(rest)?;
                Some((ServerMsg::AudioReady { seq }, rest))
            }
            1 => {
                let (ev, rest) = RadioEvent::decode(rest)?;
                Some((ServerMsg::RadioEvent(ev), rest))
            }
            2 => {
                let (rx_xruns,   rest) = read_u64(rest)?;
                let (tx_xruns,   rest) = read_u64(rest)?;
                let (dispatch_lat, rest) = LatencyStats::decode(rest)?;
                Some((ServerMsg::Stats { rx_xruns, tx_xruns, dispatch_lat }, rest))
            }
            3 => Some((ServerMsg::Pong, rest)),
            4 => {
                let (s, rest) = read_str(rest)?;
                Some((ServerMsg::Error(s), rest))
            }
            _ => None,
        }
    }
}

// ── Convenience: encode to length-prefixed framing ───────────────────────────

/// Encode a value as a 4-byte LE length-prefixed packet (same framing as the IPC layer).
pub fn to_framed<T: Encode>(msg: &T) -> Vec<u8> {
    let payload = msg.to_vec();
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// Read a length-prefixed message from a byte slice, returning the decoded
/// value and remaining bytes.  Returns `None` if the buffer is incomplete.
pub fn from_framed<T: Decode>(buf: &[u8]) -> Option<(T, &[u8])> {
    let (len, rest) = read_u32(buf)?;
    let len = len as usize;
    if rest.len() < len { return None; }
    let (val, _) = T::decode(&rest[..len])?;
    Some((val, &rest[len..]))
}

// ── Unused import suppression ─────────────────────────────────────────────────
// SAMPLES_PER_FRAME is in scope via `use super::*` for future use.
const _: usize = SAMPLES_PER_FRAME;
