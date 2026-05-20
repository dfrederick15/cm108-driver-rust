use cm108_types::{CM108_VID, CM108_PIDS, IFACE_AUDIO, IFACE_HID};
use rusb::{DeviceHandle, GlobalContext};
use tracing::info;

use crate::{HalError, Result};

pub struct Cm108Device {
    pub handle: DeviceHandle<GlobalContext>,
    pub pid: u16,
}

impl Cm108Device {
    /// Find and open the first CM108/CM119 device on the USB bus.
    /// Detaches the kernel snd-usb-audio and hid drivers so we own the device.
    pub fn open() -> Result<Self> {
        let devices = rusb::devices()?;
        for device in devices.iter() {
            let desc = device.device_descriptor()?;
            if desc.vendor_id() == CM108_VID && CM108_PIDS.contains(&desc.product_id()) {
                let mut handle = device.open()?;
                let pid = desc.product_id();

                detach_if_active(&mut handle, IFACE_AUDIO)?;
                detach_if_active(&mut handle, IFACE_HID)?;

                handle.claim_interface(IFACE_AUDIO)?;
                handle.claim_interface(IFACE_HID)?;

                info!(pid = format!("{:#06x}", pid), "opened CM108 device");
                return Ok(Self { handle, pid });
            }
        }
        Err(HalError::NotFound)
    }
}

impl Drop for Cm108Device {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(IFACE_AUDIO);
        let _ = self.handle.release_interface(IFACE_HID);
        // Re-attach kernel drivers so the device is usable again after we exit.
        let _ = self.handle.attach_kernel_driver(IFACE_AUDIO);
        let _ = self.handle.attach_kernel_driver(IFACE_HID);
    }
}

fn detach_if_active(handle: &mut DeviceHandle<GlobalContext>, iface: u8) -> Result<()> {
    match handle.kernel_driver_active(iface) {
        Ok(true) => {
            handle.detach_kernel_driver(iface)?;
            info!(iface, "detached kernel driver");
        }
        Ok(false) => {}
        // Some platforms (e.g. macOS) don't support this call — ignore.
        Err(rusb::Error::NotSupported) => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
