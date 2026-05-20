use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cm108_hal::{hid_gpio::HidGpio, rt, Cm108Device, IsoStream};
use cm108_types::GpioState;
use tracing::{info, warn};

use crate::ipc::{ClientContext, ClientRegistry};
use crate::shmem::AudioShmem;

pub fn run(socket_path: &str) -> anyhow::Result<()> {
    // ── Hardware ──────────────────────────────────────────────────────────────
    let device = Arc::new(Cm108Device::open()?);
    let gpio = Arc::new(Mutex::new(HidGpio::new()));

    // ── Shared memory (RX: server→clients) ───────────────────────────────────
    let rx_shmem = Arc::new(AudioShmem::create("cm108-rx")?);

    // ── IsoStream (USB audio threads) ─────────────────────────────────────────
    let iso = IsoStream::start(Arc::clone(&device), 90, 90, 1, 1)?;
    let IsoStream { rx_consumer, tx_producer: _tx_producer, rx_xruns, tx_xruns } = iso;

    // ── Client registry ───────────────────────────────────────────────────────
    let registry = Arc::new(ClientRegistry::new());

    // ── Audio dispatch thread: IsoStream RX → shmem seqlock → AudioReady ─────
    {
        let shmem = Arc::clone(&rx_shmem);
        let reg = Arc::clone(&registry);
        let rxr = rx_xruns.clone();
        let txr = tx_xruns.clone();
        thread::Builder::new()
            .name("cm108-dispatch".into())
            .spawn(move || {
                rt::configure_rt(88, 1);
                let mut stats_ticker = 0u64;
                let mut rx_consumer = rx_consumer;
                loop {
                    match rx_consumer.pop() {
                        Ok(frame) => {
                            let seq = shmem.write(&frame);
                            reg.notify_audio_ready(seq);
                            stats_ticker = stats_ticker.wrapping_add(1);
                            // Broadcast xrun stats every ~5 seconds (5000 frames @ 1ms each)
                            if stats_ticker % 5_000 == 0 {
                                reg.broadcast_stats(
                                    rxr.load(std::sync::atomic::Ordering::Relaxed),
                                    txr.load(std::sync::atomic::Ordering::Relaxed),
                                );
                            }
                        }
                        Err(_) => std::hint::spin_loop(),
                    }
                }
            })?;
    }

    // ── GPIO poller thread: HID interrupt-IN → RadioEvent broadcasts ──────────
    {
        let dev = Arc::clone(&device);
        let reg = Arc::clone(&registry);
        thread::Builder::new()
            .name("cm108-gpio".into())
            .spawn(move || {
                rt::configure_rt(85, 2);
                let mut prev = GpioState(0);
                loop {
                    match HidGpio::read_state(&dev.handle) {
                        Ok(state) if state != prev => {
                            emit_gpio_events(state, prev, &reg);
                            prev = state;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!("HID poll error: {e}");
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            })?;
    }

    // ── Unix socket accept loop ───────────────────────────────────────────────
    let sock_path = Path::new(socket_path);
    if sock_path.exists() {
        std::fs::remove_file(sock_path)?;
    }
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(sock_path)?;
    info!(socket = socket_path, "cm108d listening");

    let ctx = Arc::new(ClientContext {
        registry,
        rx_shmem_fd: rx_shmem.raw_fd(),
        device,
        gpio,
    });

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let ctx = Arc::clone(&ctx);
                thread::spawn(move || crate::ipc::handle_client(stream, ctx));
            }
            Err(e) => warn!("accept error: {e}"),
        }
    }

    Ok(())
}

fn emit_gpio_events(state: GpioState, prev: GpioState, reg: &ClientRegistry) {
    use cm108_types::RadioEvent;

    // GPIO1 (bit 0) → PTT
    let ptt = state.pin(0);
    if ptt != prev.pin(0) {
        reg.broadcast_radio_event(if ptt { RadioEvent::PttAssert } else { RadioEvent::PttDeassert });
    }
    // GPIO2 (bit 1) → COS
    let cos = state.pin(1);
    if cos != prev.pin(1) {
        reg.broadcast_radio_event(if cos { RadioEvent::CosActive } else { RadioEvent::CosInactive });
    }
    reg.broadcast_radio_event(RadioEvent::GpioChange(state));
}
