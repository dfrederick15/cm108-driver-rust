use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};

use cm108_types::{AudioFrame, FRAME_BYTES, SAMPLES_PER_FRAME};

/// One OS page — enough for the seqlock header + one AudioFrame + padding.
/// Layout: [u64 seq_counter (8 bytes)][AudioFrame data (192 bytes)][padding to 4096]
const SHMEM_SIZE: usize = 4096;

/// Seqlock-protected shared memory region backed by a `memfd`.
/// The server writes; clients map it read-only via the fd passed over SCM_RIGHTS.
pub struct AudioShmem {
    fd: OwnedFd,
    ptr: *mut u8,
}

// SAFETY: the mmap region is valid for the lifetime of AudioShmem; we never
// hand out non-atomic references to the seqlock header.
unsafe impl Send for AudioShmem {}
unsafe impl Sync for AudioShmem {}

impl AudioShmem {
    pub fn create(label: &str) -> std::io::Result<Self> {
        let fd = create_memfd(label)?;
        ftruncate(fd.as_raw_fd(), SHMEM_SIZE)?;
        let ptr = mmap_shared(fd.as_raw_fd(), SHMEM_SIZE)?;
        Ok(Self { fd, ptr })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Write one AudioFrame using a seqlock. Returns the new (even) seq number.
    pub fn write(&self, frame: &AudioFrame) -> u64 {
        let seq = self.seq();
        let cur = seq.load(Ordering::Relaxed);
        seq.store(cur.wrapping_add(1), Ordering::Release); // odd → write in progress
        unsafe {
            std::ptr::copy_nonoverlapping(
                frame.0.as_ptr().cast::<u8>(),
                self.data_ptr(),
                FRAME_BYTES,
            );
        }
        let new = cur.wrapping_add(2);
        seq.store(new, Ordering::Release); // even → stable
        new
    }

    pub fn current_seq(&self) -> u64 {
        self.seq().load(Ordering::Acquire)
    }

    fn seq(&self) -> &AtomicU64 {
        // SAFETY: mmap returns page-aligned (≥ 8-byte aligned) memory.
        unsafe { &*(self.ptr.cast::<AtomicU64>()) }
    }

    fn data_ptr(&self) -> *mut u8 {
        // SAFETY: seq occupies bytes 0–7; data starts at byte 8.
        unsafe { self.ptr.add(8) }
    }
}

impl Drop for AudioShmem {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr.cast(), SHMEM_SIZE) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    impl AudioShmem {
        /// Seqlock read — mirrors what the client-side `RxShmem::read_frame` does.
        fn read_frame_seqlock(&self) -> AudioFrame {
            loop {
                let s1 = self.seq().load(Ordering::Acquire);
                if s1 % 2 != 0 { std::hint::spin_loop(); continue; }
                let mut frame = AudioFrame::default();
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.data_ptr() as *const u8,
                        frame.0.as_mut_ptr().cast::<u8>(),
                        FRAME_BYTES,
                    );
                }
                if self.seq().load(Ordering::Acquire) == s1 { return frame; }
            }
        }
    }

    #[test]
    fn write_and_read_single_frame() {
        let shmem = AudioShmem::create("test-rw").unwrap();
        let mut frame = AudioFrame::default();
        frame.0[0] = 0x1234;
        frame.0[95] = -1;
        let seq = shmem.write(&frame);
        assert_eq!(seq % 2, 0, "seq must be even after write");
        let got = shmem.read_frame_seqlock();
        assert_eq!(got.0[0], 0x1234);
        assert_eq!(got.0[95], -1);
    }

    #[test]
    fn seq_increments_by_two_per_write() {
        let shmem = AudioShmem::create("test-seq").unwrap();
        let s1 = shmem.write(&AudioFrame::default());
        let s2 = shmem.write(&AudioFrame::default());
        assert_eq!(s2, s1 + 2);
    }

    #[test]
    fn seqlock_no_torn_reads() {
        // Writer alternates between frames where every sample == toggle value.
        // Reader verifies every sample in each read frame is identical (no mixing).
        let shmem = Arc::new(AudioShmem::create("test-torn").unwrap());
        let stop = Arc::new(AtomicBool::new(false));

        let w_shmem = Arc::clone(&shmem);
        let w_stop = Arc::clone(&stop);
        let writer = std::thread::spawn(move || {
            let mut toggle = 0i16;
            while !w_stop.load(Ordering::Relaxed) {
                let frame = AudioFrame([toggle; SAMPLES_PER_FRAME * 2]);
                w_shmem.write(&frame);
                toggle = toggle.wrapping_add(1);
            }
        });

        for _ in 0..20_000 {
            let frame = shmem.read_frame_seqlock();
            let expected = frame.0[0];
            for (i, &s) in frame.0.iter().enumerate() {
                assert_eq!(
                    s, expected,
                    "torn seqlock read: sample[{i}] = {s}, expected {expected}"
                );
            }
        }

        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
    }
}

// ── low-level helpers (use libc directly to avoid nix version uncertainty) ──

fn create_memfd(name: &str) -> std::io::Result<OwnedFd> {
    let cname = std::ffi::CString::new(name).expect("name must not contain NUL");
    // SAFETY: standard syscall.
    let fd = unsafe {
        libc::syscall(libc::SYS_memfd_create, cname.as_ptr(), libc::MFD_CLOEXEC) as i32
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn ftruncate(fd: RawFd, size: usize) -> std::io::Result<()> {
    // SAFETY: standard syscall; fd is valid.
    if unsafe { libc::ftruncate(fd, size as libc::off_t) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn mmap_shared(fd: RawFd, size: usize) -> std::io::Result<*mut u8> {
    // SAFETY: fd is a valid memfd of at least `size` bytes.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ptr.cast())
}
