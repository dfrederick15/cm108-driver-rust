pub mod device;
pub mod hid_gpio;
pub mod iso_stream;
pub mod rt;
pub mod usbfs;

pub use device::Cm108Device;
pub use hid_gpio::HidGpio;
pub use iso_stream::IsoStream;

use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

// ── Minimal structured logger ─────────────────────────────────────────────────
// Level encoding: 0=trace, 1=debug, 2=info, 3=warn, 4=error

static LOG_LEVEL: AtomicU8 = AtomicU8::new(2); // default: info

/// Set the log level for this crate (0=trace … 4=error).
pub fn set_log_level(level: u8) {
    LOG_LEVEL.store(level, Ordering::Relaxed);
}

macro_rules! log_info {
    ($($t:tt)*) => {
        if $crate::LOG_LEVEL.load(::std::sync::atomic::Ordering::Relaxed) <= 2 {
            ::std::eprintln!("[INFO]  {}", ::std::format_args!($($t)*));
        }
    };
}
macro_rules! log_warn {
    ($($t:tt)*) => {
        if $crate::LOG_LEVEL.load(::std::sync::atomic::Ordering::Relaxed) <= 3 {
            ::std::eprintln!("[WARN]  {}", ::std::format_args!($($t)*));
        }
    };
}
macro_rules! log_debug {
    ($($t:tt)*) => {
        if $crate::LOG_LEVEL.load(::std::sync::atomic::Ordering::Relaxed) <= 1 {
            ::std::eprintln!("[DEBUG] {}", ::std::format_args!($($t)*));
        }
    };
}

// Make the macros available to submodules via `use crate::log_*`.
pub(crate) use log_info;
pub(crate) use log_warn;
pub(crate) use log_debug;

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum HalError {
    Usb(rusb::Error),
    NotFound,
    Hid(String),
    Xrun,
}

impl fmt::Display for HalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usb(e)   => write!(f, "USB error: {e}"),
            Self::NotFound => write!(f, "no CM108/CM119 device found"),
            Self::Hid(s)   => write!(f, "HID report error: {s}"),
            Self::Xrun     => write!(f, "ring buffer full — xrun"),
        }
    }
}

impl std::error::Error for HalError {}

impl From<rusb::Error> for HalError {
    fn from(e: rusb::Error) -> Self { Self::Usb(e) }
}

pub type Result<T> = std::result::Result<T, HalError>;
