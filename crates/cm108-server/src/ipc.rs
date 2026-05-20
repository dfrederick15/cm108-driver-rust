use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cm108_types::{ClientMsg, RadioEvent, ServerMsg, StreamFlags};
use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
use nix::sys::socket::UnixAddr;
use tracing::{debug, warn};

// ── Client registry ──────────────────────────────────────────────────────────

pub type ClientId = u64;

pub struct ClientHandle {
    pub id: ClientId,
    pub streams: StreamFlags,
    /// Sends outbound ServerMsg to this client's writer thread.
    pub sender: std::sync::mpsc::SyncSender<ServerMsg>,
}

pub struct ClientRegistry {
    clients: Mutex<HashMap<ClientId, ClientHandle>>,
    next_id: AtomicU64,
}

impl ClientRegistry {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn allocate_id(&self) -> ClientId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn register(&self, handle: ClientHandle) {
        self.clients.lock().unwrap().insert(handle.id, handle);
    }

    pub fn unregister(&self, id: ClientId) {
        self.clients.lock().unwrap().remove(&id);
        debug!(id, "client disconnected");
    }

    pub fn update_streams(&self, id: ClientId, streams: StreamFlags) {
        if let Some(h) = self.clients.lock().unwrap().get_mut(&id) {
            h.streams = streams;
        }
    }

    /// Send a RadioEvent to every client subscribed to GPIO_EVENTS.
    pub fn broadcast_radio_event(&self, event: RadioEvent) {
        let clients = self.clients.lock().unwrap();
        for h in clients.values() {
            if h.streams.contains(StreamFlags::GPIO_EVENTS) {
                let _ = h.sender.try_send(ServerMsg::RadioEvent(event));
            }
        }
    }

    /// Notify every AUDIO_IN subscriber that a new frame is available at `seq`.
    pub fn notify_audio_ready(&self, seq: u64) {
        let clients = self.clients.lock().unwrap();
        for h in clients.values() {
            if h.streams.contains(StreamFlags::AUDIO_IN) {
                let _ = h.sender.try_send(ServerMsg::AudioReady { seq });
            }
        }
    }

    pub fn broadcast_stats(&self, rx_xruns: u64, tx_xruns: u64) {
        let clients = self.clients.lock().unwrap();
        for h in clients.values() {
            let _ = h.sender.try_send(ServerMsg::Stats { rx_xruns, tx_xruns });
        }
    }
}

// ── Per-client connection handler ────────────────────────────────────────────

pub struct ClientContext {
    pub registry: Arc<ClientRegistry>,
    pub rx_shmem_fd: RawFd,
    pub device: Arc<cm108_hal::Cm108Device>,
    pub gpio: Arc<Mutex<cm108_hal::HidGpio>>,
}

pub fn handle_client(mut stream: UnixStream, ctx: Arc<ClientContext>) {
    let id = ctx.registry.allocate_id();
    let (msg_tx, msg_rx) = std::sync::mpsc::sync_channel::<ServerMsg>(128);

    ctx.registry.register(ClientHandle {
        id,
        streams: StreamFlags::empty(),
        sender: msg_tx,
    });

    // Step 1: hand the RX shmem fd to the client via SCM_RIGHTS.
    if let Err(e) = send_fd(stream.as_raw_fd(), ctx.rx_shmem_fd) {
        warn!(id, "failed to send shmem fd: {e}");
        ctx.registry.unregister(id);
        return;
    }

    // Step 2: writer thread — serialises outbound ServerMsg onto the socket.
    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            warn!(id, "try_clone failed: {e}");
            ctx.registry.unregister(id);
            return;
        }
    };
    std::thread::spawn(move || {
        for msg in msg_rx {
            if write_msg(&mut writer, &msg).is_err() {
                break;
            }
        }
    });

    debug!(id, "client connected");

    // Step 3: reader loop — process inbound ClientMsg.
    loop {
        match read_msg(&mut stream) {
            Ok(Some(msg)) => dispatch_client_msg(id, msg, &ctx),
            Ok(None) => break,
            Err(e) => {
                warn!(id, "read error: {e}");
                break;
            }
        }
    }

    ctx.registry.unregister(id);
}

fn dispatch_client_msg(id: ClientId, msg: ClientMsg, ctx: &ClientContext) {
    match msg {
        ClientMsg::Subscribe { streams } => {
            ctx.registry.update_streams(id, streams);
            debug!(id, ?streams, "client subscribed");
        }
        ClientMsg::SetGpio { pin, high } => {
            if let Ok(mut g) = ctx.gpio.lock() {
                if let Err(e) = g.set_pin(&ctx.device.handle, pin, high) {
                    warn!(id, pin, high, "SetGpio error: {e}");
                }
            }
        }
        ClientMsg::AudioWrite { frame_count } => {
            // Phase 4: read frame_count frames from TX shmem, push to IsoStream.
            debug!(id, frame_count, "AudioWrite (unimplemented in Phase 3)");
        }
        ClientMsg::Ping => {
            // Enqueue pong; writer thread will flush it.
            let clients = ctx.registry.clients.lock().unwrap();
            if let Some(h) = clients.get(&id) {
                let _ = h.sender.try_send(ServerMsg::Pong);
            }
        }
    }
}

// ── Message framing (4-byte LE length prefix + postcard payload) ─────────────

pub fn write_msg(stream: &mut impl Write, msg: &ServerMsg) -> io::Result<()> {
    let payload = postcard::to_allocvec(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = (payload.len() as u32).to_le_bytes();
    stream.write_all(&len)?;
    stream.write_all(&payload)?;
    stream.flush()
}

pub fn read_msg(stream: &mut impl Read) -> io::Result<Option<ClientMsg>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 65_536 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    postcard::from_bytes(&buf)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ── SCM_RIGHTS fd passing ────────────────────────────────────────────────────

/// Send `fd_to_send` as ancillary data over `socket` (connected Unix stream).
fn send_fd(socket: RawFd, fd_to_send: RawFd) -> io::Result<()> {
    let payload = [0u8; 1]; // Linux requires ≥ 1 byte of regular data with SCM_RIGHTS
    let iov = [std::io::IoSlice::new(&payload)];
    let fds = [fd_to_send];
    let cmsg = [ControlMessage::ScmRights(&fds)];
    let none: Option<&UnixAddr> = None;
    sendmsg(socket, &iov, &cmsg, MsgFlags::empty(), none)
        .map(|_| ())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}
