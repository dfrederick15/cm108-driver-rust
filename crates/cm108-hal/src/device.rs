use cm108_types::{CM108_VID, CM108_PIDS, IFACE_AUDIO_CTRL, IFACE_AUDIO_OUT, IFACE_AUDIO_IN, IFACE_HID};
use rusb::{DeviceHandle, GlobalContext};

use crate::{log_info, HalError, Result};

pub struct Cm108Device {
    pub handle: DeviceHandle<GlobalContext>,
    pub pid: u16,
    /// USB bus number — used by IsoStream to open /dev/bus/usb/BBB/DDD.
    pub bus: u8,
    /// USB device address on the bus.
    pub address: u8,
}

impl Cm108Device {
    pub fn open() -> Result<Self> {
        let devices = rusb::devices()?;
        for device in devices.iter() {
            let desc = device.device_descriptor()?;
            if desc.vendor_id() == CM108_VID && CM108_PIDS.contains(&desc.product_id()) {
                let mut handle = device.open()?;
                let pid = desc.product_id();
                let bus = device.bus_number();
                let address = device.address();

                // Detach kernel drivers from all interfaces so the process owns the device.
                for iface in [IFACE_AUDIO_CTRL, IFACE_AUDIO_OUT, IFACE_AUDIO_IN, IFACE_HID] {
                    detach_if_active(&mut handle, iface)?;
                }

                // rusb claims audio-control and HID; IsoStream claims streaming interfaces.
                handle.claim_interface(IFACE_AUDIO_CTRL)?;
                handle.claim_interface(IFACE_HID)?;

                log_info!("opened CM108 device pid={pid:#06x} bus={bus} addr={address}");
                return Ok(Self { handle, pid, bus, address });
            }
        }
        Err(HalError::NotFound)
    }
}

impl Drop for Cm108Device {
    fn drop(&mut self) {
        for iface in [IFACE_AUDIO_CTRL, IFACE_HID] {
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
