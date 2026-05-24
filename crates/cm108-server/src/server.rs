use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cm108_hal::{hid_gpio::HidGpio, rt, Cm108Device, IsoStream};
use cm108_types::{GpioState, LatencyStats};

use crate::ipc::{ClientContext, ClientRegistry};
use crate::latency::LatencyHistogram;
use crate::shmem::AudioShmem;

// GPIO4 (pin index 3) — PC_OK heartbeat LED (inverting: high → LED on).
const HEARTBEAT_PIN: u8 = 3;

pub fn run(socket_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let device = Arc::new(Cm108Device::open()?);
    let gpio = Arc::new(Mutex::new(HidGpio::new()));
    let rx_shmem = Arc::new(AudioShmem::create("cm108-rx")?);
    let iso = IsoStream::start(Arc::clone(&device), 90, 90, 1, 1)?;
    let IsoStream { rx_consumer, tx_producer, rx_xruns, tx_xruns } = iso;

    let registry = Arc::new(ClientRegistry::new());
    let last_latency: Arc<Mutex<LatencyStats>> =
        Arc::new(Mutex::new(LatencyStats::default()));

    // ── Audio dispatch thread ─────────────────────────────────────────────────
    {
        let shmem = Arc::clone(&rx_shmem);
        let reg   = Arc::clone(&registry);
        let rxr   = Arc::clone(&rx_xruns);
        let txr   = Arc::clone(&tx_xruns);
        let lat   = Arc::clone(&last_latency);
        thread::Builder::new()
            .name("cm108-dispatch".into())
            .spawn(move || {
                rt::configure_rt(88, 1);
                let mut ticker = 0u64;
                let mut rx_consumer = rx_consumer;
                let mut histogram = LatencyHistogram::new();
                loop {
                    match rx_consumer.pop() {
                        Ok(frame) => {
                            let t0 = Instant::now();
                            let seq = shmem.write(&frame);
                            reg.notify_audio_ready(seq);
                            histogram.record(t0.elapsed().as_micros() as u32);
                            ticker = ticker.wrapping_add(1);
                            if ticker % 5_000 == 0 {
                                let snap = histogram.to_stats();
                                *lat.lock().unwrap() = snap;
                                reg.broadcast_stats(
                                    rxr.load(Ordering::Relaxed),
                                    txr.load(Ordering::Relaxed),
                                    snap,
                                );
                                histogram.reset();
                            }
                        }
                        Err(_) => thread::sleep(Duration::from_millis(1)),
                    }
                }
            })?;
    }

    // ── GPIO poller thread ────────────────────────────────────────────────────
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
                        Ok(Some(state)) if state != prev => {
                            emit_gpio_events(state, prev, &reg);
                            prev = state;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            log_warn!("HID poll error: {e}");
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            })?;
    }

    // ── Heartbeat LED thread ──────────────────────────────────────────────────
    // 1 Hz when daemon is alive, 2 Hz when a client is actively sending messages.
    let last_activity_ms = Arc::new(AtomicU64::new(0));
    {
        let dev  = Arc::clone(&device);
        let gpio = Arc::clone(&gpio);
        let act  = Arc::clone(&last_activity_ms);
        thread::Builder::new()
            .name("cm108-heartbeat".into())
            .spawn(move || {
                let mut pin_state = false;
                loop {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let since_ms = now_ms.saturating_sub(act.load(Ordering::Relaxed));
                    // Within 2 s of last activity → 2 Hz (250 ms half-period).
                    // Otherwise → 1 Hz (500 ms half-period).
                    let half_period = if since_ms < 2_000 {
                        Duration::from_millis(250)
                    } else {
                        Duration::from_millis(500)
                    };
                    thread::sleep(half_period);
                    pin_state = !pin_state;
                    if let Ok(mut g) = gpio.lock() {
                        let _ = g.set_pin(&dev.handle, HEARTBEAT_PIN, pin_state);
                    }
                }
            })?;
    }

    // ── Unix socket accept loop ───────────────────────────────────────────────
    let sock_path = Path::new(socket_path);
    if sock_path.exists() { std::fs::remove_file(sock_path)?; }
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent)?;
        // Allow non-root clients (e.g. the 'repeater' user) to enter the dir.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            parent,
            std::fs::Permissions::from_mode(0o755),
        );
    }
    let listener = UnixListener::bind(sock_path)?;
    log_info!("cm108d listening socket={socket_path}");

    let ctx = Arc::new(ClientContext {
        registry,
        rx_shmem_fd: rx_shmem.raw_fd(),
        device,
        gpio,
        tx_producer: Mutex::new(tx_producer),
        rx_xruns,
        tx_xruns,
        last_latency,
        last_activity_ms,
    });

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let ctx = Arc::clone(&ctx);
                thread::spawn(move || crate::ipc::handle_client(stream, ctx));
            }
            Err(e) => log_warn!("accept error: {e}"),
        }
    }

    Ok(())
}

fn emit_gpio_events(state: GpioState, prev: GpioState, reg: &ClientRegistry) {
    use cm108_types::RadioEvent;

    let ptt = state.pin(0);
    if ptt != prev.pin(0) {
        reg.broadcast_radio_event(if ptt { RadioEvent::PttAssert } else { RadioEvent::PttDeassert });
    }
    let cos = state.pin(1);
    if cos != prev.pin(1) {
        reg.broadcast_radio_event(if cos { RadioEvent::CosActive } else { RadioEvent::CosInactive });
    }
    reg.broadcast_radio_event(RadioEvent::GpioChange(state));
}
