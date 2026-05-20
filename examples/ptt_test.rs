//! Smoke test: toggle GPIO1 (PTT) at 1 Hz for 10 seconds.
//! Verify with a multimeter or LED on the GPIO1 pin.

use std::thread::sleep;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let device = cm108_hal::Cm108Device::open()?;
    let mut gpio = cm108_hal::HidGpio::new();

    for i in 0..10 {
        let high = i % 2 == 0;
        gpio.set_pin(&device.handle, 0, high)?;
        println!("PTT {}", if high { "ASSERT" } else { "DEASSERT" });
        sleep(Duration::from_secs(1));
    }

    gpio.set_pin(&device.handle, 0, false)?;
    Ok(())
}
