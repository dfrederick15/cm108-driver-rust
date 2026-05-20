//! Audio loopback: pipe ISO-IN directly to ISO-OUT for 60 seconds.
//! Connect audio-out to audio-in physically; verify no xruns.

use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let device = Arc::new(cm108_hal::Cm108Device::open()?.handle);
    let mut stream = cm108_hal::IsoStream::start(
        Arc::clone(&device),
        80, 80, 1, 1,
    )?;

    let start = Instant::now();
    let mut frames = 0u64;

    while start.elapsed() < Duration::from_secs(60) {
        if let Ok(frame) = stream.rx_consumer.pop() {
            let _ = stream.tx_producer.push(frame);
            frames += 1;
        }
    }

    println!(
        "frames={frames} rx_xruns={} tx_xruns={}",
        stream.rx_xruns.load(std::sync::atomic::Ordering::Relaxed),
        stream.tx_xruns.load(std::sync::atomic::Ordering::Relaxed),
    );
    Ok(())
}
