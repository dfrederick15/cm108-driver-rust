#![cfg_attr(not(feature = "std"), no_std)]

pub mod codec;
pub use codec::{Decode, Encode};

// ── USB constants ─────────────────────────────────────────────────────────────

pub const CM108_VID: u16 = 0x0d8c;

/// Known CM108/CM119 product IDs.
pub const CM108_PIDS: &[u16] = &[
    0x001f, 0x0105, 0x0107, 0x010f, 0x0115, 0x013a, 0x013c,
];

pub const IFACE_AUDIO_CTRL: u8 = 0; // USB Audio Class: audio control
pub const IFACE_AUDIO_OUT:  u8 = 1; // audio streaming — speaker (ISO OUT)
pub const IFACE_AUDIO_IN:   u8 = 2; // audio streaming — mic (ISO IN)
pub const IFACE_HID:        u8 = 3; // HID interface — GPIO/PTT
pub const EP_ISO_OUT:       u8 = 0x01;
pub const EP_ISO_IN:        u8 = 0x82;
pub const EP_HID_IN:        u8 = 0x87;

/// Bytes per USB frame: 48 samples × stereo × i16 @ 48 kHz = 1 ms.
pub const FRAME_BYTES: usize = 192;
pub const SAMPLES_PER_FRAME: usize = 48;

// ── Audio frame ───────────────────────────────────────────────────────────────

/// One millisecond of 48 kHz / 16-bit stereo audio.
/// Layout: [L0, R0, L1, R1, …] interleaved i16, little-endian.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(4))]
pub struct AudioFrame(pub [i16; SAMPLES_PER_FRAME * 2]);

impl Default for AudioFrame {
    fn default() -> Self { Self([0i16; SAMPLES_PER_FRAME * 2]) }
}

// ── GPIO ──────────────────────────────────────────────────────────────────────

/// Bitmask of GPIO1–GPIO4 pin states (bit 0 = GPIO1 / PTT).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpioState(pub u8);

impl GpioState {
    pub fn pin(self, n: u8) -> bool {
        debug_assert!(n < 4, "CM108 has GPIO1-GPIO4 only");
        self.0 & (1 << n) != 0
    }
}

// ── Radio events ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioEvent {
    PttAssert,
    PttDeassert,
    CosActive,
    CosInactive,
    GpioChange(GpioState),
}

// ── Dispatch latency summary ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LatencyStats {
    pub min_us: u32,
    pub max_us: u32,
    pub p99_us: u32,
}

// ── Stream subscription flags ─────────────────────────────────────────────────

/// Bitmask of audio/event streams a client subscribes to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct StreamFlags(pub u8);

impl StreamFlags {
    pub const AUDIO_IN:    Self = Self(0b0001);
    pub const AUDIO_OUT:   Self = Self(0b0010);
    pub const GPIO_EVENTS: Self = Self(0b0100);

    pub fn empty() -> Self { Self(0) }
    pub fn bits(self) -> u8 { self.0 }
    pub fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
    pub fn from_bits_truncate(v: u8) -> Self { Self(v & 0b0111) }
}

impl std::ops::BitOr for StreamFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

impl std::ops::BitOrAssign for StreamFlags {
    fn bitor_assign(&mut self, rhs: Self) { self.0 |= rhs.0; }
}

// ── IPC protocol ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum ClientMsg {
    Subscribe { streams: StreamFlags },
    SetGpio { pin: u8, high: bool },
    AudioWrite { frame_count: u32 },
    Ping,
    GetStats,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerMsg {
    AudioReady { seq: u64 },
    RadioEvent(RadioEvent),
    Stats {
        rx_xruns: u64,
        tx_xruns: u64,
        dispatch_lat: LatencyStats,
    },
    Pong,
    Error(String),
}
