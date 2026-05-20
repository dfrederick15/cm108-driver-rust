use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use cm108_types::{AudioFrame, ClientMsg, ServerMsg, StreamFlags, FRAME_BYTES};

use crate::framing::{read_server_msg, write_client_msg};
use crate::{ClientError, Result};

const SHMEM_SIZE: usize = 4096;

// ── Read-only client mapping of the server's AudioShmem ──────────────────────

struct RxShmem {
    ptr: *const u8,
    _fd: OwnedFd, // keep fd alive as long as the mapping lives
}

unsafe impl Send for RxShmem {}
unsafe impl Sync for RxShmem {}

impl RxShmem {
    fn map(fd: OwnedFd) -> Result<Self> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SHMEM_SIZE,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(ClientError::Io(std::io::Error::last_os_error()));
        }
        Ok(Self { ptr: ptr.cast(), _fd: fd })
    }

    fn seq(&self) -> &AtomicU64 {
        // SAFETY: mmap returns page-aligned memory; AtomicU64 requires 8-byte alignment.
        unsafe { &*(self.ptr.cast::<AtomicU64>()) }
    }

    /// Seqlock read: spin until the seq counter is even (stable) and consistent.
    pub fn read_frame(&self) -> AudioFrame {
        let seq = self.seq();
        loop {
            let s1 = seq.load(Ordering::Acquire);
            if s1 % 2 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let mut frame = AudioFrame::default();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.ptr.add(8),          // data starts after 8-byte seq
                    frame.0.as_mut_ptr().cast::<u8>(),
                    FRAME_BYTES,
                );
            }
            if seq.load(Ordering::Acquire) == s1 {
                return frame;
            }
        }
    }

    pub fn current_seq(&self) -> u64 {
        self.seq().load(Ordering::Acquire)
    }
}

impl Drop for RxShmem {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, SHMEM_SIZE) };
    }
}

// ── Cm108Client ───────────────────────────────────────────────────────────────

pub struct Cm108Client {
    /// Write-only end: sends ClientMsg to the server.
    writer: UnixStream,
    /// Read-only end: receives ServerMsg from the server.
    reader: UnixStream,
    rx_shmem: RxShmem,
    last_seq: u64,
}

impl Cm108Client {
    /// Connect to a running `cm108d` instance.
    /// The server sends the RX shmem fd as SCM_RIGHTS on the first byte.
    pub fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        let reader = stream.try_clone()?;
        let shmem_fd = recv_fd(stream.as_raw_fd())?;
        let rx_shmem = RxShmem::map(shmem_fd)?;
        Ok(Self { writer: stream, reader, rx_shmem, last_seq: 0 })
    }

    pub fn subscribe(&mut self, flags: StreamFlags) -> Result<()> {
        write_client_msg(&mut self.writer, &ClientMsg::Subscribe { streams: flags })
            .map_err(ClientError::Io)
    }

    pub fn set_ptt(&mut self, asserted: bool) -> Result<()> {
        write_client_msg(&mut self.writer, &ClientMsg::SetGpio { pin: 0, high: asserted })
            .map_err(ClientError::Io)
    }

    pub fn set_gpio(&mut self, pin: u8, high: bool) -> Result<()> {
        write_client_msg(&mut self.writer, &ClientMsg::SetGpio { pin, high })
            .map_err(ClientError::Io)
    }

    /// Block until the server sends an `AudioReady` notification, then return
    /// the corresponding frame from the seqlock shmem.
    pub fn read_audio_blocking(&mut self) -> Result<AudioFrame> {
        loop {
            match read_server_msg(&mut self.reader)? {
                Some(ServerMsg::AudioReady { seq }) if seq > self.last_seq => {
                    self.last_seq = seq;
                    return Ok(self.rx_shmem.read_frame());
                }
                Some(_) => {} // stale notification or other message — keep waiting
                None => return Err(ClientError::Disconnected),
            }
        }
    }

    /// Read the latest frame directly from shmem without waiting for a
    /// server notification. Useful for polling-based consumers.
    pub fn read_audio_latest(&self) -> AudioFrame {
        self.rx_shmem.read_frame()
    }

    pub fn current_rx_seq(&self) -> u64 {
        self.rx_shmem.current_seq()
    }

    /// Non-blocking: return the next server message if one is immediately
    /// available on the socket, or `None` if the socket would block.
    pub fn poll_msg(&mut self) -> Result<Option<ServerMsg>> {
        self.reader.set_nonblocking(true)?;
        let res = read_server_msg(&mut self.reader);
        self.reader.set_nonblocking(false)?;
        match res {
            Ok(msg) => Ok(msg),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(ClientError::Io(e)),
        }
    }

    /// Blocking: wait for the next server message.
    pub fn wait_msg(&mut self) -> Result<Option<ServerMsg>> {
        read_server_msg(&mut self.reader).map_err(ClientError::Io)
    }

    pub fn ping(&mut self) -> Result<()> {
        write_client_msg(&mut self.writer, &ClientMsg::Ping).map_err(ClientError::Io)
    }
}

// ── SCM_RIGHTS receive (via raw libc, avoids nix version variance) ───────────

fn recv_fd(socket: i32) -> Result<OwnedFd> {
    let cmsg_space =
        unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize };
    let mut cmsg_buf = vec![0u8; cmsg_space];
    let mut payload = [0u8; 1];

    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: payload.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov as *mut _;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr().cast();
    msg.msg_controllen = cmsg_space as _;

    if unsafe { libc::recvmsg(socket, &mut msg, 0) } < 0 {
        return Err(ClientError::Io(std::io::Error::last_os_error()));
    }

    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !cmsg.is_null() {
        if unsafe { (*cmsg).cmsg_level == libc::SOL_SOCKET
            && (*cmsg).cmsg_type == libc::SCM_RIGHTS }
        {
            let fd = unsafe { *(libc::CMSG_DATA(cmsg).cast::<i32>()) };
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }
    Err(ClientError::Protocol(
        "server did not send shmem fd on connect".into(),
    ))
}
