//! Print RadioEvents (PTT/COS transitions) to stdout indefinitely.

use std::sync::{Arc, Mutex};
use cm108_hal::{Cm108Device, hid_gpio};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let device = Cm108Device::open()?;
    let handle = Arc::new(Mutex::new(device.handle));
    let rx = hid_gpio::spawn_gpio_poller(handle, 85, 2);

    println!("Monitoring GPIO events (Ctrl-C to stop)…");
    for event in rx {
        println!("{event:?}");
    }
    Ok(())
}
