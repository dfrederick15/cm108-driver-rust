use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use cm108_types::{AudioFrame, FRAME_BYTES, EP_ISO_IN, EP_ISO_OUT};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::{log_debug, log_warn, Cm108Device, Result};

const RING_CAPACITY: usize = 64;

pub struct IsoStream {
    pub rx_consumer: Consumer<AudioFrame>,
    pub tx_producer: Producer<AudioFrame>,
    pub rx_xruns: Arc<AtomicU64>,
    pub tx_xruns: Arc<AtomicU64>,
}

impl IsoStream {
    /// Spawn USB RX and TX threads. The caller gets back the ring-buffer ends
    /// facing the application, plus xrun counters.
    pub fn start(
        device: Arc<Cm108Device>,
        rx_priority: i32,
        tx_priority: i32,
        rx_core: usize,
        tx_core: usize,
    ) -> Result<Self> {
        let (rx_prod, rx_cons) = RingBuffer::<AudioFrame>::new(RING_CAPACITY);
        let (tx_prod, tx_cons) = RingBuffer::<AudioFrame>::new(RING_CAPACITY);

        let rx_xruns = Arc::new(AtomicU64::new(0));
        let tx_xruns = Arc::new(AtomicU64::new(0));

        spawn_rx_thread(Arc::clone(&device), rx_prod, Arc::clone(&rx_xruns), rx_priority, rx_core);
        spawn_tx_thread(Arc::clone(&device), tx_cons, Arc::clone(&tx_xruns), tx_priority, tx_core);

        Ok(Self { rx_consumer: rx_cons, tx_producer: tx_prod, rx_xruns, tx_xruns })
    }
}

fn spawn_rx_thread(
    device: Arc<Cm108Device>,
    mut prod: Producer<AudioFrame>,
    xruns: Arc<AtomicU64>,
    priority: i32,
    core: usize,
) {
    thread::Builder::new()
        .name("cm108-rx".into())
        .spawn(move || {
            crate::rt::configure_rt(priority, core);
            log_debug!("RX thread started");

            let mut buf = [0u8; FRAME_BYTES];
            loop {
                match device
                    .handle
                    .read_bulk(EP_ISO_IN, &mut buf, std::time::Duration::from_millis(5))
                {
                    Ok(n) if n == FRAME_BYTES => {
                        let frame = bytes_to_frame(&buf);
                        if prod.push(frame).is_err() {
                            xruns.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Ok(_) => {}
                    Err(rusb::Error::Timeout) => {}
                    Err(e) => log_warn!("RX error: {e}"),
                }
            }
        })
        .expect("failed to spawn RX thread");
}

fn spawn_tx_thread(
    device: Arc<Cm108Device>,
    mut cons: Consumer<AudioFrame>,
    xruns: Arc<AtomicU64>,
    priority: i32,
    core: usize,
) {
    thread::Builder::new()
        .name("cm108-tx".into())
        .spawn(move || {
            crate::rt::configure_rt(priority, core);
            log_debug!("TX thread started");

            loop {
                match cons.pop() {
                    Ok(frame) => {
                        let buf = frame_to_bytes(&frame);
                        if let Err(e) = device.handle.write_bulk(
                            EP_ISO_OUT,
                            &buf,
                            std::time::Duration::from_millis(5),
                        ) {
                            if e != rusb::Error::Timeout {
                                log_warn!("TX error: {e}");
                                xruns.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(_) => std::hint::spin_loop(),
                }
            }
        })
        .expect("failed to spawn TX thread");
}

fn bytes_to_frame(buf: &[u8; FRAME_BYTES]) -> AudioFrame {
    let mut frame = AudioFrame::default();
    for (i, sample) in frame.0.iter_mut().enumerate() {
        let lo = buf[i * 2] as i16;
        let hi = buf[i * 2 + 1] as i16;
        *sample = lo | (hi << 8);
    }
    frame
}

fn frame_to_bytes(frame: &AudioFrame) -> [u8; FRAME_BYTES] {
    let mut buf = [0u8; FRAME_BYTES];
    for (i, &sample) in frame.0.iter().enumerate() {
        buf[i * 2]     = (sample & 0xff) as u8;
        buf[i * 2 + 1] = ((sample >> 8) & 0xff) as u8;
    }
    buf
}
