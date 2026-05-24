use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cm108_types::{ClientMsg, LatencyStats, RadioEvent, ServerMsg, StreamFlags};

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
        log_debug!("client disconnected id={id}");
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

    pub fn broadcast_stats(&self, rx_xruns: u64, tx_xruns: u64, dispatch_lat: LatencyStats) {
        let clients = self.clients.lock().unwrap();
        for h in clients.values() {
            let _ = h.sender.try_send(ServerMsg::Stats { rx_xruns, tx_xruns, dispatch_lat });
        }
    }

    fn send_to(&self, id: ClientId, msg: ServerMsg) {
        let clients = self.clients.lock().unwrap();
        if let Some(h) = clients.get(&id) {
            let _ = h.sender.try_send(msg);
        }
    }
}

// ── Per-client connection handler ────────────────────────────────────────────

pub struct ClientContext {
    pub registry: Arc<ClientRegistry>,
    pub rx_shmem_fd: RawFd,
    pub device: Arc<cm108_hal::Cm108Device>,
    pub gpio: Arc<Mutex<cm108_hal::HidGpio>>,
    pub rx_xruns: Arc<AtomicU64>,
    pub tx_xruns: Arc<AtomicU64>,
    pub last_latency: Arc<Mutex<LatencyStats>>,
    pub last_activity_ms: Arc<AtomicU64>,
}

pub fn handle_client(mut stream: UnixStream, ctx: Arc<ClientContext>) {
    let id = ctx.registry.allocate_id();
    let (msg_tx, msg_rx) = std::sync::mpsc::sync_channel::<ServerMsg>(128);

    ctx.registry.register(ClientHandle {
        id,
        streams: StreamFlags::empty(),
        sender: msg_tx,
    });

    if let Err(e) = send_fd(stream.as_raw_fd(), ctx.rx_shmem_fd) {
        log_warn!("failed to send shmem fd: {e} id={id}");
        ctx.registry.unregister(id);
        return;
    }

    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            log_warn!("try_clone failed: {e} id={id}");
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

    log_debug!("client connected id={id}");

    loop {
        match read_msg(&mut stream) {
            Ok(Some(msg)) => dispatch_client_msg(id, msg, &ctx),
            Ok(None) => break,
            Err(e) => {
                log_warn!("read error: {e} id={id}");
                break;
            }
        }
    }

    ctx.registry.unregister(id);
}

fn dispatch_client_msg(id: ClientId, msg: ClientMsg, ctx: &ClientContext) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    ctx.last_activity_ms.store(now_ms, Ordering::Relaxed);

    match msg {
        ClientMsg::Subscribe { streams } => {
            ctx.registry.update_streams(id, streams);
            log_debug!("client subscribed id={id} streams={streams:?}");
        }
        ClientMsg::SetGpio { pin, high } => {
            if let Ok(mut g) = ctx.gpio.lock() {
                if let Err(e) = g.set_pin(&ctx.device.handle, pin, high) {
                    log_warn!("SetGpio error: {e} id={id} pin={pin} high={high}");
                }
            }
        }
        ClientMsg::AudioWrite { frame_count } => {
            log_debug!("AudioWrite id={id} frame_count={frame_count} (TX path not yet implemented)");
        }
        ClientMsg::Ping => {
            ctx.registry.send_to(id, ServerMsg::Pong);
        }
        ClientMsg::GetStats => {
            let lat = ctx.last_latency.lock().unwrap().clone();
            ctx.registry.send_to(id, ServerMsg::Stats {
                rx_xruns: ctx.rx_xruns.load(Ordering::Relaxed),
                tx_xruns: ctx.tx_xruns.load(Ordering::Relaxed),
                dispatch_lat: lat,
            });
        }
    }
}

// ── Message framing (4-byte LE length prefix + hand-rolled codec payload) ────

pub fn write_msg(stream: &mut impl Write, msg: &ServerMsg) -> io::Result<()> {
    use cm108_types::Encode;
    let payload = msg.to_vec();
    stream.write_all(&(payload.len() as u32).to_le_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()
}

pub fn read_msg(stream: &mut impl Read) -> io::Result<Option<ClientMsg>> {
    use cm108_types::Decode;
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
    ClientMsg::from_bytes(&buf)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "decode error"))
        .map(Some)
}

// ── SCM_RIGHTS fd passing via raw libc ───────────────────────────────────────

/// Send `fd_to_send` as SCM_RIGHTS ancillary data over `socket`.
fn send_fd(socket: RawFd, fd_to_send: RawFd) -> io::Result<()> {
    let cmsg_space =
        unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize };
    let mut cmsg_buf = vec![0u8; cmsg_space];
    let payload = [0u8; 1]; // Linux requires ≥1 byte of real data with SCM_RIGHTS

    let mut iov = libc::iovec {
        iov_base: payload.as_ptr() as *mut libc::c_void,
        iov_len:  payload.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov        = &mut iov as *mut _;
    msg.msg_iovlen     = 1;
    msg.msg_control    = cmsg_buf.as_mut_ptr().cast();
    msg.msg_controllen = cmsg_space as _;

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type  = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len   =
            libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as _;
        *(libc::CMSG_DATA(cmsg).cast::<i32>()) = fd_to_send;
    }

    if unsafe { libc::sendmsg(socket, &msg, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
