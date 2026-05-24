use cm108_types::{CM108_VID, CM108_PIDS, IFACE_AUDIO_CTRL, IFACE_AUDIO_OUT, IFACE_AUDIO_IN, IFACE_HID};
use rusb::{DeviceHandle, GlobalContext};

use crate::{log_info, HalError, Result};

pub struct Cm108Device {
    pub handle: DeviceHandle<GlobalContext>,
    pub pid: u16,
}

impl Cm108Device {
    /// Find and open the first CM108/CM119 device on the USB bus.
    /// Detaches snd-usb-audio and hid from all four interfaces, claims them,
    /// then activates alternate setting 1 on the two streaming interfaces.
    pub fn open() -> Result<Self> {
        let devices = rusb::devices()?;
        for device in devices.iter() {
            let desc = device.device_descriptor()?;
            if desc.vendor_id() == CM108_VID && CM108_PIDS.contains(&desc.product_id()) {
                let mut handle = device.open()?;
                let pid = desc.product_id();

                for iface in [IFACE_AUDIO_CTRL, IFACE_AUDIO_OUT, IFACE_AUDIO_IN, IFACE_HID] {
                    detach_if_active(&mut handle, iface)?;
                    handle.claim_interface(iface)?;
                }

                // Activate isochronous endpoints (alt 0 has no endpoints).
                handle.set_alternate_setting(IFACE_AUDIO_OUT, 1).map_err(HalError::Usb)?;
                handle.set_alternate_setting(IFACE_AUDIO_IN,  1).map_err(HalError::Usb)?;

                log_info!("opened CM108 device pid={pid:#06x}");
                return Ok(Self { handle, pid });
            }
        }
        Err(HalError::NotFound)
    }
}

impl Drop for Cm108Device {
    fn drop(&mut self) {
        for iface in [IFACE_AUDIO_CTRL, IFACE_AUDIO_OUT, IFACE_AUDIO_IN, IFACE_HID] {
            let _ = self.handle.set_alternate_setting(iface, 0);
            let _ = self.handle.release_interface(iface);
            let _ = self.handle.attach_kernel_driver(iface);
        }
    }
}

fn detach_if_active(handle: &mut DeviceHandle<GlobalContext>, iface: u8) -> Result<()> {
    match handle.kernel_driver_active(iface) {
        Ok(true) => {
            handle.detach_kernel_driver(iface)?;
            log_info!("detached kernel driver iface={iface}");
        }
        Ok(false) => {}
        Err(rusb::Error::NotSupported) => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
