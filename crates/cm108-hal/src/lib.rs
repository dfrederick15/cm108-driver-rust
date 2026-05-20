pub mod device;
pub mod hid_gpio;
pub mod iso_stream;
pub mod rt;

pub use device::Cm108Device;
pub use hid_gpio::HidGpio;
pub use iso_stream::IsoStream;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HalError {
    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),
    #[error("no CM108/CM119 device found")]
    NotFound,
    #[error("HID report error: {0}")]
    Hid(String),
    #[error("ring buffer full — xrun")]
    Xrun,
}

pub type Result<T> = std::result::Result<T, HalError>;
