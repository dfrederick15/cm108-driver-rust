#![cfg_attr(not(feature = "std"), no_std)]

use serde::{Deserialize, Serialize};

// ── USB constants ─────────────────────────────────────────────────────────────

pub const CM108_VID: u16 = 0x0d8c;

/// Known CM108/CM119 product IDs.
pub const CM108_PIDS: &[u16] = &[
    0x001f, 0x0105, 0x0107, 0x010f, 0x0115, 0x013c,
];

/// Audio control interface index.
pub const IFACE_AUDIO: u8 = 0;
/// HID interface index (GPIO).
pub const IFACE_HID: u8 = 2;

/// Isochronous audio-OUT endpoint (host → device).
pub const EP_ISO_OUT: u8 = 0x01;
/// Isochronous audio-IN endpoint (device → host).
pub const EP_ISO_IN: u8 = 0x82;
/// Interrupt-IN endpoint for HID GPIO reports.
pub const EP_HID_IN: u8 = 0x83;

/// Bytes per USB frame: 48 samples × stereo × i16 @ 48 kHz = 1 ms.
pub const FRAME_BYTES: usize = 192;
/// Samples per frame per channel.
pub const SAMPLES_PER_FRAME: usize = 48;

// ── Audio frame ───────────────────────────────────────────────────────────────

/// One millisecond of 48 kHz / 16-bit stereo audio.
/// Layout: [L0, R0, L1, R1, …] interleaved i16, little-endian.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(4))]
pub struct AudioFrame(pub [i16; SAMPLES_PER_FRAME * 2]);

impl Default for AudioFrame {
    fn default() -> Self {
        Self([0i16; SAMPLES_PER_FRAME * 2])
    }
}

// ── GPIO ──────────────────────────────────────────────────────────────────────

/// Bitmask of GPIO1–GPIO4 pin states (bit 0 = GPIO1 / PTT).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpioState(pub u8);

impl GpioState {
    pub fn pin(self, n: u8) -> bool {
        debug_assert!(n < 4, "CM108 has GPIO1-GPIO4 only");
        self.0 & (1 << n) != 0
    }
}

// ── Radio events ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadioEvent {
    PttAssert,
    PttDeassert,
    CosActive,
    CosInactive,
    GpioChange(GpioState),
}

// ── IPC protocol ─────────────────────────────────────────────────────────────

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct StreamFlags: u8 {
        const AUDIO_IN    = 0b0001;
        const AUDIO_OUT   = 0b0010;
        const GPIO_EVENTS = 0b0100;
    }
}

/// Dispatch latency summary reported by the server every 5 000 frames (~5 s).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyStats {
    pub min_us: u32,
    pub max_us: u32,
    /// Approximate 99th-percentile dispatch latency (µs), log2-bucket resolution.
    pub p99_us: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    Subscribe { streams: StreamFlags },
    SetGpio { pin: u8, high: bool },
    /// Client has written `frame_count` frames to the TX shmem region.
    AudioWrite { frame_count: u32 },
    Ping,
    /// Request an immediate stats snapshot from the server.
    GetStats,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// `seq` is the frame sequence number written to RX shmem.
    AudioReady { seq: u64 },
    RadioEvent(RadioEvent),
    /// Health counters and dispatch latency histogram summary.
    Stats {
        rx_xruns: u64,
        tx_xruns: u64,
        dispatch_lat: LatencyStats,
    },
    Pong,
    Error(#[serde(with = "serde_string")] String),
}

mod serde_string {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(s: &str, ser: S) -> Result<S::Ok, S::Error> {
        s.serialize(ser)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
        String::deserialize(de)
    }
}
