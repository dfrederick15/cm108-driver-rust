use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cm108_types::{AudioFrame, FRAME_BYTES, EP_ISO_IN, EP_ISO_OUT, IFACE_AUDIO_IN, IFACE_AUDIO_OUT};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::usbfs::{
    self, SingleIsoUrb, UsbdevfsUrb, UsbIsoPacketDesc, URB_TYPE_ISO,
};
use crate::{log_debug, log_warn, Cm108Device, HalError, Result};

const RING_CAPACITY: usize = 64;

// Number of URBs kept in flight simultaneously — gives N ms of pipeline depth.
const N_URBS: usize = 8;

// CM119 descriptor: ISO IN max packet = 100 bytes (mono 48kHz 16-bit = 96 bytes + slack)
const ISO_IN_PACKET: usize = 100;

pub struct IsoStream {
    pub rx_consumer: Consumer<AudioFrame>,
    pub tx_producer: Producer<AudioFrame>,
    pub rx_xruns:    Arc<AtomicU64>,
    pub tx_xruns:    Arc<AtomicU64>,
}

impl IsoStream {
    pub fn start(
        device:      Arc<Cm108Device>,
        rx_priority: i32,
        tx_priority: i32,
        rx_core:     usize,
        tx_core:     usize,
    ) -> Result<Self> {
        let (rx_prod, rx_cons) = RingBuffer::<AudioFrame>::new(RING_CAPACITY);
        let (tx_prod, tx_cons) = RingBuffer::<AudioFrame>::new(RING_CAPACITY);
        let rx_xruns = Arc::new(AtomicU64::new(0));
        let tx_xruns = Arc::new(AtomicU64::new(0));

        spawn_rx_thread(Arc::clone(&device), rx_prod, Arc::clone(&rx_xruns), rx_priority, rx_core)?;
        spawn_tx_thread(Arc::clone(&device), tx_cons, Arc::clone(&tx_xruns), tx_priority, tx_core)?;

        Ok(Self { rx_consumer: rx_cons, tx_producer: tx_prod, rx_xruns, tx_xruns })
    }
}

// ── RX thread ────────────────────────────────────────────────────────────────

fn spawn_rx_thread(
    device:   Arc<Cm108Device>,
    prod:     Producer<AudioFrame>,
    xruns:    Arc<AtomicU64>,
    priority: i32,
    core:     usize,
) -> Result<()> {
    thread::Builder::new()
        .name("cm108-rx".into())
        .spawn(move || {
            crate::rt::configure_rt(priority, core);

            let file = match usbfs::open_usbfs(device.bus, device.address) {
                Ok(f) => f,
                Err(e) => { log_warn!("RX: open usbfs failed: {e}"); return; }
            };
            let fd = usbfs::usbfs_fd(&file);

            if let Err(e) = usbfs::claim_interface(fd, IFACE_AUDIO_IN as u32)
                .and_then(|_| usbfs::set_interface(fd, IFACE_AUDIO_IN as u32, 1))
            {
                log_warn!("RX: setup failed: {e}"); return;
            }

            // Allocate N URBs, each with one ISO packet.
            let mut urbs: Vec<Box<SingleIsoUrb>> = (0..N_URBS)
                .map(|_| Box::new(unsafe { std::mem::zeroed::<SingleIsoUrb>() }))
                .collect();
            let mut bufs: Vec<Box<[u8; ISO_IN_PACKET]>> = (0..N_URBS)
                .map(|_| Box::new([0u8; ISO_IN_PACKET]))
                .collect();

            // Submit all URBs initially.
            for i in 0..N_URBS {
                init_rx_urb(&mut urbs[i], &mut bufs[i], i);
                if let Err(e) = unsafe { usbfs::submit_urb(fd, &mut *urbs[i]) } {
                    log_warn!("RX: initial submit failed i={i}: {e}"); return;
                }
            }

            log_debug!("RX thread started");
            let mut prod = prod;

            loop {
                match usbfs::reap_urb(fd) {
                    Ok(ctx) => {
                        let i = ctx as usize;
                        if i < N_URBS {
                            let n = urbs[i].pkt.actual_length as usize;
                            if urbs[i].hdr.status == 0 && n > 0 {
                                let frame = bytes_to_frame(&bufs[i][..n]);
                                if prod.push(frame).is_err() {
                                    xruns.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            // Resubmit immediately.
                            init_rx_urb(&mut urbs[i], &mut bufs[i], i);
                            if let Err(e) = unsafe { usbfs::submit_urb(fd, &mut *urbs[i]) } {
                                log_warn!("RX: resubmit failed: {e}");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_micros(200));
                    }
                    Err(e) => {
                        log_warn!("RX: reap error: {e}");
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        })
        .map_err(|e| HalError::Hid(e.to_string()))?;
    Ok(())
}

fn init_rx_urb(urb: &mut SingleIsoUrb, buf: &mut [u8; ISO_IN_PACKET], idx: usize) {
    urb.hdr = UsbdevfsUrb {
        typ:               URB_TYPE_ISO,
        endpoint:          EP_ISO_IN,
        _pad0:             0,
        status:            0,
        flags:             0,
        _pad1:             0,
        buffer:            buf.as_mut_ptr().cast(),
        buffer_length:     ISO_IN_PACKET as i32,
        actual_length:     0,
        start_frame:       0,
        number_of_packets: 1,
        error_count:       0,
        signr:             0,
        usercontext:       idx as *mut libc::c_void,
    };
    urb.pkt = UsbIsoPacketDesc {
        length:        ISO_IN_PACKET as u32,
        actual_length: 0,
        status:        0,
    };
}

// ── TX thread ────────────────────────────────────────────────────────────────

fn spawn_tx_thread(
    device:   Arc<Cm108Device>,
    cons:     Consumer<AudioFrame>,
    xruns:    Arc<AtomicU64>,
    priority: i32,
    core:     usize,
) -> Result<()> {
    thread::Builder::new()
        .name("cm108-tx".into())
        .spawn(move || {
            crate::rt::configure_rt(priority, core);

            let file = match usbfs::open_usbfs(device.bus, device.address) {
                Ok(f) => f,
                Err(e) => { log_warn!("TX: open usbfs failed: {e}"); return; }
            };
            let fd = usbfs::usbfs_fd(&file);

            if let Err(e) = usbfs::claim_interface(fd, IFACE_AUDIO_OUT as u32)
                .and_then(|_| usbfs::set_interface(fd, IFACE_AUDIO_OUT as u32, 1))
            {
                log_warn!("TX: setup failed: {e}"); return;
            }

            let mut urbs: Vec<Box<SingleIsoUrb>> = (0..N_URBS)
                .map(|_| Box::new(unsafe { std::mem::zeroed::<SingleIsoUrb>() }))
                .collect();
            // Each TX buffer is FRAME_BYTES = 192.
            let mut bufs: Vec<Box<[u8; FRAME_BYTES]>> = (0..N_URBS)
                .map(|_| Box::new([0u8; FRAME_BYTES]))
                .collect();

            // Pre-fill and submit all URBs with silence.
            for i in 0..N_URBS {
                init_tx_urb(&mut urbs[i], &mut bufs[i], i);
                if let Err(e) = unsafe { usbfs::submit_urb(fd, &mut *urbs[i]) } {
                    log_warn!("TX: initial submit failed i={i}: {e}"); return;
                }
            }

            log_debug!("TX thread started");
            let mut cons = cons;

            loop {
                match usbfs::reap_urb(fd) {
                    Ok(ctx) => {
                        let i = ctx as usize;
                        if i < N_URBS {
                            // Fill buffer from ring or silence.
                            match cons.pop() {
                                Ok(frame) => frame_to_bytes(&frame, &mut bufs[i]),
                                Err(_) => {
                                    xruns.fetch_add(1, Ordering::Relaxed);
                                    bufs[i].fill(0);
                                }
                            }
                            init_tx_urb(&mut urbs[i], &mut bufs[i], i);
                            if let Err(e) = unsafe { usbfs::submit_urb(fd, &mut *urbs[i]) } {
                                log_warn!("TX: resubmit failed: {e}");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_micros(200));
                    }
                    Err(e) => {
                        log_warn!("TX: reap error: {e}");
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            }
        })
        .map_err(|e| HalError::Hid(e.to_string()))?;
    Ok(())
}

fn init_tx_urb(urb: &mut SingleIsoUrb, buf: &mut [u8; FRAME_BYTES], idx: usize) {
    urb.hdr = UsbdevfsUrb {
        typ:               URB_TYPE_ISO,
        endpoint:          EP_ISO_OUT,
        _pad0:             0,
        status:            0,
        flags:             0,
        _pad1:             0,
        buffer:            buf.as_mut_ptr().cast(),
        buffer_length:     FRAME_BYTES as i32,
        actual_length:     0,
        start_frame:       0,
        number_of_packets: 1,
        error_count:       0,
        signr:             0,
        usercontext:       idx as *mut libc::c_void,
    };
    urb.pkt = UsbIsoPacketDesc {
        length:        FRAME_BYTES as u32,
        actual_length: 0,
        status:        0,
    };
}

// ── audio format helpers ─────────────────────────────────────────────────────

/// Convert raw bytes from the CM119 to an AudioFrame.
/// The RX endpoint sends mono 48kHz 16-bit LE; we duplicate to stereo.
fn bytes_to_frame(buf: &[u8]) -> AudioFrame {
    let mut frame = AudioFrame::default();
    let samples = buf.len() / 2;
    let count = samples.min(cm108_types::SAMPLES_PER_FRAME);
    for i in 0..count {
        let s = i16::from_le_bytes([buf[i * 2], buf[i * 2 + 1]]);
        frame.0[i * 2]     = s; // L
        frame.0[i * 2 + 1] = s; // R (mono duplicated)
    }
    frame
}

fn frame_to_bytes(frame: &AudioFrame, buf: &mut [u8; FRAME_BYTES]) {
    for (i, &sample) in frame.0.iter().enumerate() {
        let le = sample.to_le_bytes();
        buf[i * 2]     = le[0];
        buf[i * 2 + 1] = le[1];
    }
}
