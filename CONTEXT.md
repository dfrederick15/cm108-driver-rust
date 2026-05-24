# cm108-driver — Project Context

## What This Is

A pure-Rust CM108/CM119 USB audio driver daemon (`cm108d`) for a ham radio repeater controller.
It replaces the kernel's `snd_usb_audio` and `hid` drivers with a direct userspace driver that
owns the device exclusively. Deployed on a Raspberry Pi 5 (aarch64, Debian 13 trixie).

**GitHub:** https://github.com/dfrederick15/cm108-driver-rust  
**Pi address:** repeaterd@172.16.11.188  
**Binary on Pi:** `/usr/sbin/cm108d`  
**Socket:** `/run/cm108d/cm108d.sock`

---

## Hardware

**Board:** Repeater-Builder RB_RIM_Lite_v2 (N3XCC design)  
**Chip:** C-Media CM119, USB VID:PID `0d8c:013a`

### CM119 USB Interface Layout (0d8c:013a)

| Interface | Type            | Alt | Endpoint | Dir | Max Pkt |
|-----------|-----------------|-----|----------|-----|---------|
| 0         | Audio Control   | 0   | —        | —   | —       |
| 1         | Audio Streaming | 1   | 0x01     | OUT | 200 B   |
| 2         | Audio Streaming | 1   | 0x82     | IN  | 100 B   |
| 3         | HID             | 0   | 0x87     | IN  | 4 B     |

- **EP 0x01** ISO OUT — TX audio to radio (speaker), 192 bytes/ms (48kHz stereo 16-bit)
- **EP 0x82** ISO IN  — RX audio from radio (mic), 96–100 bytes/ms (**mono** 48kHz 16-bit)
- **EP 0x87** Interrupt IN — GPIO/PTT/COS state, 4 bytes, 20ms poll

### GPIO Mapping (RB_RIM_Lite_v2)

| GPIO | Direction | Function              | Notes                                     |
|------|-----------|-----------------------|-------------------------------------------|
| 1    | Output    | PTT TX key            | High = key transmitter                    |
| 2    | Input     | COS (carrier squelch) | From radio receiver                       |
| 3    | Input     | CTCSS/COS composite   | See Note B on schematic                   |
| 4    | Output    | PC_OK heartbeat LED   | Inverted via transistor: GPIO4 high → LED on |

Direction register: `0x09` (bits 0 and 3 = outputs, bits 1 and 2 = inputs).

**Heartbeat behavior:** GPIO4 toggles on every IPC message received from a connected client.
This makes the LED blink proportionally to client activity. LED is off when no client is communicating.

---

## Architecture

```
cm108d (server)
├── rusb owns:   interface 0 (audio control) + interface 3 (HID/GPIO)
│   └── EP 0x87 — interrupt reads, 20ms timeout, silence = Ok(None)
└── usbfs owns:  interface 1 (audio OUT) + interface 2 (audio IN)
    ├── /dev/bus/usb/003/002 — opened directly via USBDEVFS ioctls
    ├── EP 0x01 — 8 ISO OUT URBs in flight (USBDEVFS_SUBMITURB)
    └── EP 0x82 — 8 ISO IN  URBs in flight (USBDEVFS_SUBMITURB)

Threads:
  cm108-rx      — ISO IN reap loop → rtrb ring → cm108-dispatch
  cm108-tx      — rtrb ring → ISO OUT submit loop (silence on underrun)
  cm108-dispatch — ring consumer → shmem write → notify AUDIO_IN clients
  cm108-gpio    — HID interrupt reads → broadcast RadioEvent to GPIO_EVENTS clients
  cm108d accept — Unix socket accept loop, spawns per-client handler threads
```

### IPC Protocol

Unix socket at `/run/cm108d/cm108d.sock`. On connect, server sends shmem fd via SCM_RIGHTS.
Messages framed as 4-byte LE length + hand-rolled binary payload (no serde/postcard).

**ClientMsg tags:** 0=Subscribe, 1=SetGpio, 2=AudioWrite, 3=Ping, 4=GetStats  
**ServerMsg tags:** 0=AudioReady, 1=RadioEvent, 2=Stats, 3=Pong, 4=Error

Subscribe flags: `0x01`=AUDIO_IN, `0x02`=AUDIO_OUT, `0x04`=GPIO_EVENTS

---

## Crate Structure

```
cm108-driver/
├── crates/
│   ├── cm108-types/     — shared types + hand-rolled codec (no serde)
│   │   └── src/
│   │       ├── lib.rs   — StreamFlags, AudioFrame, RadioEvent, GpioState, etc.
│   │       └── codec.rs — Encode/Decode traits, wire format impls
│   ├── cm108-hal/       — hardware abstraction (rusb + raw usbfs)
│   │   └── src/
│   │       ├── lib.rs        — logger macros, HalError, set_log_level()
│   │       ├── device.rs     — Cm108Device::open() — detach ALSA, claim ifaces
│   │       ├── hid_gpio.rs   — HidGpio: read_state() → Option<GpioState>
│   │       ├── iso_stream.rs — IsoStream: 8-URB ISO RX/TX via usbfs
│   │       ├── usbfs.rs      — raw USBDEVFS ioctl wrappers
│   │       └── rt.rs         — SCHED_FIFO + CPU affinity + mlockall
│   ├── cm108-server/    — cm108d daemon binary
│   │   └── src/
│   │       ├── main.rs    — arg parse (no clap), log init, calls run()
│   │       ├── server.rs  — thread spawning, Unix socket accept loop
│   │       ├── ipc.rs     — per-client handler, ClientRegistry, heartbeat toggle
│   │       ├── latency.rs — latency histogram (lock-free)
│   │       └── shmem.rs   — seqlock shmem via memfd + SCM_RIGHTS
│   └── cm108-client/    — Rust client library + C FFI (libcm108client)
│       └── src/
│           ├── lib.rs      — cm108_connect/destroy/subscribe/read_audio/etc.
│           ├── client.rs   — UnixStream + recv_fd (SCM_RIGHTS)
│           └── framing.rs  — write_msg / read_msg using codec
├── include/cm108.h      — C header (hand-written, not cbindgen)
├── udev/90-cm108.rules  — installed at /etc/udev/rules.d/ on Pi
└── systemd/cm108d.service
```

---

## Dependencies (intentionally minimal)

Only 3 external runtime crates — everything else hand-rolled:

| Crate  | Why kept                                                    |
|--------|-------------------------------------------------------------|
| `rusb` | USB enumeration, interface claim, HID interrupt transfers   |
| `rtrb` | Lock-free SPSC ring buffer (correctness-critical, ARM safe) |
| `libc` | POSIX types, USBDEVFS ioctls, mmap, memfd, SCM_RIGHTS       |

Eliminated: `anyhow`, `thiserror`, `bitflags`, `nix`, `clap`, `tracing`,
`tracing-subscriber`, `postcard`, `serde`, `cbindgen`.

---

## Deployment

### Cross-compile (from dev machine at /home/debian/projects/cm108-driver)

```bash
PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig \
    ~/.cargo/bin/cargo build --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/cm108d repeaterd@172.16.11.188:/tmp/cm108d
ssh repeaterd@172.16.11.188 'sudo cp /tmp/cm108d /usr/sbin/cm108d'
```

### Start daemon

```bash
sudo cm108d --socket /run/cm108d/cm108d.sock --log info
```

Env vars: `CM108_SOCKET`, `CM108_LOG` (trace/debug/info/warn/error).

### On reboot

`snd_usb_audio` reclaims the device but `cm108d` detaches all 4 interfaces via
`libusb_detach_kernel_driver` at startup — no manual intervention needed.
The udev rule at `/etc/udev/rules.d/90-cm108.rules` prevents rebinding on hotplug.

---

## Current Status

| Feature                        | Status  | Notes                                      |
|--------------------------------|---------|--------------------------------------------|
| USB device open + ALSA detach  | ✅ Works | All 4 interfaces detached on startup       |
| HID GPIO polling (PTT/COS)     | ✅ Works | EP 0x87, 20ms timeout, silence = no event  |
| GPIO output (PTT key)          | ✅ Works | cm108_set_ptt() / cm108_set_gpio()         |
| Heartbeat LED (GPIO4/PC_OK)    | ✅ Works | Toggles on each IPC message from client    |
| GPIO direction (RIM Lite v2)   | ✅ Fixed | 0x09: GPIO1+GPIO4 out, GPIO2+GPIO3 in      |
| ISO RX audio (from radio)      | ✅ Works | 8 URBs, mono→stereo, ~10% CPU             |
| ISO TX audio (to radio)        | ✅ Works | 8 URBs, silence on underrun               |
| Shmem audio path to clients    | ✅ Works | seqlock memfd, fd via SCM_RIGHTS           |
| C FFI client library           | ✅ Works | libcm108client.so + include/cm108.h        |
| Systemd service                | ⬜ Todo  | Binary placed, service not yet enabled     |
| ISO audio format validation    | ⬜ Todo  | Assumed mono in — verify with actual audio |

---

## Known Issues / Open Items

1. **Systemd service not enabled** — `cm108d.service` exists in the repo but has not been
   `systemctl enable`d on the Pi. Daemon is currently started manually.

2. **TX ISO packet size** — TX URBs submit 192 bytes (stereo FRAME_BYTES). The CM119
   descriptor says max 200 bytes. If the device rejects 192-byte packets, try 196 or 200
   with zero-padding.

3. **Mono RX assumption** — RX endpoint max packet is 100 bytes. Code assumes mono
   (48 samples × 2 bytes = 96 bytes) and duplicates to stereo. Verify with real audio.

4. **`rusb` replacement** — Replacing rusb with raw `USBDEVFS_*` ioctls (for HID and
   control transfers too) would eliminate the last non-libc dependency. Deferred.

5. **`rtrb` replacement** — Lock-free SPSC ring could be hand-rolled, but risk of
   subtle memory ordering bugs on ARM. Deferred until audio path validated.
