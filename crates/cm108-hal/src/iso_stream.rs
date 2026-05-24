use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cm108_types::{AudioFrame, EP_ISO_IN, EP_ISO_OUT, IFACE_AUDIO_IN, IFACE_AUDIO_OUT};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::usbfs::{
    self, SingleIsoUrb, UsbdevfsUrb, UsbIsoPacketDesc, URB_TYPE_ISO,
};
use crate::{log_debug, log_warn, Cm108Device, HalError, Result};

const RING_CAPACITY: usize = 64;

// Number of URBs kept in flight simultaneously — gives N ms of pipeline depth.
const N_URBS: usize = 8;

// CM119: all audio is mono 48kHz 16-bit.
// RX max packet = 100 bytes (96 bytes = 48 samples + up to 4 bytes adaptive slack).
// TX packet = 96 bytes (48 mono samples; max 200 bytes per descriptor).
const ISO_IN_PACKET:  usize = 100;
const ISO_OUT_PACKET: usize = 96;

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
            let mut bufs: Vec<Box<[u8; ISO_OUT_PACKET]>> = (0..N_URBS)
                .map(|_| Box::new([0u8; ISO_OUT_PACKET]))
                .collect();

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
                            match cons.pop() {
                                Ok(frame) => frame_to_mono(&frame, &mut bufs[i]),
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

fn init_tx_urb(urb: &mut SingleIsoUrb, buf: &mut [u8; ISO_OUT_PACKET], idx: usize) {
    urb.hdr = UsbdevfsUrb {
        typ:               URB_TYPE_ISO,
        endpoint:          EP_ISO_OUT,
        _pad0:             0,
        status:            0,
        flags:             0,
        _pad1:             0,
        buffer:            buf.as_mut_ptr().cast(),
        buffer_length:     ISO_OUT_PACKET as i32,
        actual_length:     0,
        start_frame:       0,
        number_of_packets: 1,
        error_count:       0,
        signr:             0,
        usercontext:       idx as *mut libc::c_void,
    };
    urb.pkt = UsbIsoPacketDesc {
        length:        ISO_OUT_PACKET as u32,
        actual_length: 0,
        status:        0,
    };
}

// ── audio format helpers ─────────────────────────────────────────────────────

/// CM119 RX: mono 48kHz 16-bit LE → AudioFrame (L=R, mono duplicated to stereo).
fn bytes_to_frame(buf: &[u8]) -> AudioFrame {
    let mut frame = AudioFrame::default();
    let count = (buf.len() / 2).min(cm108_types::SAMPLES_PER_FRAME);
    for i in 0..count {
        let s = i16::from_le_bytes([buf[i * 2], buf[i * 2 + 1]]);
        frame.0[i * 2]     = s;
        frame.0[i * 2 + 1] = s;
    }
    frame
}

/// CM119 TX: AudioFrame → mono 48kHz 16-bit LE (left channel only).
fn frame_to_mono(frame: &AudioFrame, buf: &mut [u8; ISO_OUT_PACKET]) {
    for i in 0..cm108_types::SAMPLES_PER_FRAME {
        let le = frame.0[i * 2].to_le_bytes(); // left channel
        buf[i * 2]     = le[0];
        buf[i * 2 + 1] = le[1];
    }
}
