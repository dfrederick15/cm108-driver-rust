/// C-compatible FFI surface for `libcm108client`.
/// The generated header lives at `include/cm108.h` (via cbindgen).
use std::ffi::c_char;
use std::path::Path;

use cm108_types::{RadioEvent, ServerMsg, StreamFlags, SAMPLES_PER_FRAME};

use crate::Cm108Client;

// ── Event type ────────────────────────────────────────────────────────────────

/// C-compatible radio/GPIO event.
#[repr(C)]
pub struct Cm108Event {
    /// 0 = PttAssert, 1 = PttDeassert, 2 = CosActive,
    /// 3 = CosInactive, 4 = GpioChange
    pub event_type: u8,
    /// GPIO1–GPIO4 bitmask (populated for GpioChange; 0 otherwise).
    pub gpio_state: u8,
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/// Connect to a running `cm108d` server.
/// Returns an opaque handle, or NULL on failure.
/// The caller must free it with `cm108_destroy`.
#[no_mangle]
pub extern "C" fn cm108_connect(socket_path: *const c_char) -> *mut Cm108Client {
    if socket_path.is_null() {
        return std::ptr::null_mut();
    }
    let path_str = unsafe { std::ffi::CStr::from_ptr(socket_path) };
    let path = match path_str.to_str() {
        Ok(s) => Path::new(s),
        Err(_) => return std::ptr::null_mut(),
    };
    match Cm108Client::connect(path) {
        Ok(c) => Box::into_raw(Box::new(c)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Free a client handle returned by `cm108_connect`.
#[no_mangle]
pub extern "C" fn cm108_destroy(client: *mut Cm108Client) {
    if !client.is_null() {
        drop(unsafe { Box::from_raw(client) });
    }
}

// ── Subscriptions ─────────────────────────────────────────────────────────────

/// Subscribe to event/audio streams.
/// `flags` bitmask: bit 0 = AUDIO_IN, bit 1 = AUDIO_OUT, bit 2 = GPIO_EVENTS.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn cm108_subscribe(client: *mut Cm108Client, flags: u8) -> i32 {
    let c = ffi_client_mut!(client);
    match c.subscribe(StreamFlags::from_bits_truncate(flags)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

// ── Audio I/O ─────────────────────────────────────────────────────────────────

/// Block until the next audio frame is available, then copy it into `buf`.
/// `buf` must hold at least `frames` × 2 × sizeof(int16_t) bytes (stereo interleaved).
/// Returns the number of stereo frames copied (≤ `frames`), or -1 on error.
#[no_mangle]
pub extern "C" fn cm108_read_audio(
    client: *mut Cm108Client,
    buf: *mut i16,
    frames: usize,
) -> i32 {
    if buf.is_null() {
        return -1;
    }
    let c = ffi_client_mut!(client);
    match c.read_audio_blocking() {
        Ok(frame) => {
            let n = frames.min(SAMPLES_PER_FRAME);
            unsafe {
                std::ptr::copy_nonoverlapping(frame.0.as_ptr(), buf, n * 2);
            }
            n as i32
        }
        Err(_) => -1,
    }
}

/// Write audio frames for TX playback through the device.
/// (TX shmem path not yet implemented; returns `frames` as a no-op.)
#[no_mangle]
pub extern "C" fn cm108_write_audio(
    _client: *mut Cm108Client,
    _buf: *const i16,
    frames: usize,
) -> i32 {
    frames as i32
}

// ── GPIO control ──────────────────────────────────────────────────────────────

/// Assert (`asserted` != 0) or deassert PTT on GPIO1.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn cm108_set_ptt(client: *mut Cm108Client, asserted: i32) -> i32 {
    let c = ffi_client_mut!(client);
    match c.set_ptt(asserted != 0) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Set an arbitrary GPIO pin (0 = GPIO1 … 3 = GPIO4).
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn cm108_set_gpio(client: *mut Cm108Client, pin: u8, high: i32) -> i32 {
    let c = ffi_client_mut!(client);
    match c.set_gpio(pin, high != 0) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

// ── Event polling ─────────────────────────────────────────────────────────────

/// Non-blocking poll for a server event (RadioEvent or Stats).
/// Returns 1 if `*out` was filled, 0 if no event is pending, -1 on error.
#[no_mangle]
pub extern "C" fn cm108_poll_event(client: *mut Cm108Client, out: *mut Cm108Event) -> i32 {
    if out.is_null() {
        return -1;
    }
    let c = ffi_client_mut!(client);
    match c.poll_msg() {
        Ok(Some(msg)) => match server_msg_to_event(msg) {
            Some(ev) => {
                unsafe { *out = ev };
                1
            }
            None => 0,
        },
        Ok(None) => 0,
        Err(_) => -1,
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Guard macro: cast a nullable C pointer to `&mut Cm108Client` or return -1.
macro_rules! ffi_client_mut {
    ($ptr:expr) => {
        match unsafe { $ptr.as_mut() } {
            Some(c) => c,
            None => return -1,
        }
    };
}
use ffi_client_mut;

fn server_msg_to_event(msg: ServerMsg) -> Option<Cm108Event> {
    if let ServerMsg::RadioEvent(ev) = msg {
        Some(radio_to_c(ev))
    } else {
        None
    }
}

fn radio_to_c(ev: RadioEvent) -> Cm108Event {
    match ev {
        RadioEvent::PttAssert       => Cm108Event { event_type: 0, gpio_state: 0 },
        RadioEvent::PttDeassert     => Cm108Event { event_type: 1, gpio_state: 0 },
        RadioEvent::CosActive       => Cm108Event { event_type: 2, gpio_state: 0 },
        RadioEvent::CosInactive     => Cm108Event { event_type: 3, gpio_state: 0 },
        RadioEvent::GpioChange(g)   => Cm108Event { event_type: 4, gpio_state: g.0 },
    }
}
