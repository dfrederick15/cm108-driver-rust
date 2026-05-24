/// Raw Linux USBDEVFS ioctls for isochronous audio transfers.
///
/// rusb exposes only bulk/interrupt/control transfers in its safe API.
/// Isochronous transfers require USBDEVFS_SUBMITURB + USBDEVFS_REAPURBNDELAY,
/// which we call directly via libc::ioctl on /dev/bus/usb/BBB/DDD.
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

// ── ioctl numbers (64-bit Linux, aarch64 / x86_64) ──────────────────────────
//
// Computed with: _IOR/_IOW macros from <asm/ioctl.h>
// sizeof(usbdevfs_urb) = 56 on 64-bit (see UsbdevfsUrb below)
// sizeof(void*)        = 8
// sizeof(unsigned int) = 4
// sizeof(usbdevfs_setinterface) = 8

pub const USBDEVFS_SUBMITURB:     libc::c_ulong = 0x8038_550a;
pub const USBDEVFS_REAPURBNDELAY: libc::c_ulong = 0x4008_550d;
pub const USBDEVFS_CLAIMINTF:     libc::c_ulong = 0x8004_550f;
pub const USBDEVFS_SETINTF:       libc::c_ulong = 0x8008_5504;
pub const USBDEVFS_DISCARDURB:    libc::c_ulong = 0x0000_550b;

pub const URB_TYPE_ISO: u8 = 0;

// ── C structs (must match kernel ABI exactly) ────────────────────────────────

#[repr(C)]
pub struct UsbdevfsUrb {
    pub typ:               u8,
    pub endpoint:          u8,
    pub _pad0:             u16,
    pub status:            i32,
    pub flags:             u32,
    pub _pad1:             u32,
    pub buffer:            *mut libc::c_void,
    pub buffer_length:     i32,
    pub actual_length:     i32,
    pub start_frame:       i32,
    pub number_of_packets: i32,
    pub error_count:       i32,
    pub signr:             u32,
    pub usercontext:       *mut libc::c_void,
    // iso_frame_desc[number_of_packets] follows in memory
}

// Verify struct size at compile time — must be 56 on 64-bit Linux.
const _: () = assert!(std::mem::size_of::<UsbdevfsUrb>() == 56);

#[repr(C)]
pub struct UsbIsoPacketDesc {
    pub length:        u32,
    pub actual_length: u32,
    pub status:        u32,
}

#[repr(C)]
pub struct UsbdevfsSetInterface {
    pub interface:  u32,
    pub altsetting: u32,
}

/// An ISO URB with a single packet descriptor embedded immediately after the
/// header (satisfying the kernel's `iso_frame_desc[0]` layout requirement).
#[repr(C)]
pub struct SingleIsoUrb {
    pub hdr: UsbdevfsUrb,
    pub pkt: UsbIsoPacketDesc,
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Open /dev/bus/usb/<bus>/<addr> (O_RDWR | O_CLOEXEC).
pub fn open_usbfs(bus: u8, addr: u8) -> io::Result<File> {
    let path = format!("/dev/bus/usb/{bus:03}/{addr:03}");
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(&path)
}

pub fn claim_interface(fd: i32, iface: u32) -> io::Result<()> {
    let n = iface;
    if unsafe { libc::ioctl(fd, USBDEVFS_CLAIMINTF, &n as *const u32) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn set_interface(fd: i32, iface: u32, alt: u32) -> io::Result<()> {
    let s = UsbdevfsSetInterface { interface: iface, altsetting: alt };
    if unsafe { libc::ioctl(fd, USBDEVFS_SETINTF, &s as *const UsbdevfsSetInterface) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Submit a URB. The kernel takes ownership until it's reaped.
/// # Safety
/// `urb` must remain valid and pinned until reaped.
pub unsafe fn submit_urb(fd: i32, urb: *mut SingleIsoUrb) -> io::Result<()> {
    if libc::ioctl(fd, USBDEVFS_SUBMITURB, urb as *mut UsbdevfsUrb) < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Non-blocking reap. Returns the usercontext pointer of the completed URB,
/// or `Err(WouldBlock)` if none are ready.
pub fn reap_urb(fd: i32) -> io::Result<*mut libc::c_void> {
    let mut ptr: *mut libc::c_void = std::ptr::null_mut();
    if unsafe { libc::ioctl(fd, USBDEVFS_REAPURBNDELAY, &mut ptr as *mut *mut libc::c_void) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ptr)
}

pub fn usbfs_fd(file: &File) -> i32 {
    file.as_raw_fd()
}
