use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use cm108_types::{GpioState, IFACE_HID, EP_HID_IN, RadioEvent};
use rusb::DeviceHandle;

use crate::{log_warn, HalError, Result};

const HID_SET_REPORT_TYPE: u8  = 0x21;
const HID_SET_REPORT_REQ:  u8  = 0x09;
const HID_OUTPUT_REPORT:   u16 = 0x0200;
const GPIO_DIR_ALL_OUT:    u8  = 0x0f;

pub struct HidGpio {
    gpio_out: u8,
    gpio_dir: u8,
}

impl HidGpio {
    pub fn new() -> Self {
        Self { gpio_out: 0, gpio_dir: GPIO_DIR_ALL_OUT }
    }

    pub fn set_pin<C: rusb::UsbContext>(
        &mut self,
        handle: &DeviceHandle<C>,
        pin: u8,
        high: bool,
    ) -> Result<()> {
        debug_assert!(pin < 4);
        if high { self.gpio_out |= 1 << pin; } else { self.gpio_out &= !(1 << pin); }
        self.write_report(handle)
    }

    pub fn read_state<C: rusb::UsbContext>(
        handle: &DeviceHandle<C>,
    ) -> Result<GpioState> {
        let mut buf = [0u8; 4];
        handle
            .read_interrupt(EP_HID_IN, &mut buf, Duration::from_millis(20))
            .map_err(HalError::Usb)?;
        Ok(GpioState(buf[1]))
    }

    fn write_report<C: rusb::UsbContext>(&self, handle: &DeviceHandle<C>) -> Result<()> {
        let report = [0x00u8, self.gpio_out, self.gpio_dir, 0x00];
        handle
            .write_control(
                HID_SET_REPORT_TYPE,
                HID_SET_REPORT_REQ,
                HID_OUTPUT_REPORT,
                IFACE_HID as u16,
                &report,
                Duration::from_millis(100),
            )
            .map_err(HalError::Usb)?;
        Ok(())
    }
}

pub fn spawn_gpio_poller<C: rusb::UsbContext + 'static>(
    handle: std::sync::Arc<std::sync::Mutex<DeviceHandle<C>>>,
    priority: i32,
    core: usize,
) -> mpsc::Receiver<RadioEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        crate::rt::configure_rt(priority, core);

        let mut prev = GpioState(0);
        loop {
            let state = {
                let h = handle.lock().unwrap();
                match HidGpio::read_state(&*h) {
                    Ok(s) => s,
                    Err(e) => {
                        log_warn!("HID read error: {e}");
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                }
            };

            if state != prev {
                if state.pin(0) != prev.pin(0) {
                    let ev = if state.pin(0) { RadioEvent::PttAssert } else { RadioEvent::PttDeassert };
                    let _ = tx.send(ev);
                }
                if state.pin(1) != prev.pin(1) {
                    let ev = if state.pin(1) { RadioEvent::CosActive } else { RadioEvent::CosInactive };
                    let _ = tx.send(ev);
                }
                let _ = tx.send(RadioEvent::GpioChange(state));
                prev = state;
            }
        }
    });
    rx
}
