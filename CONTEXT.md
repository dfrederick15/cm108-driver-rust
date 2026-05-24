# cm108-driver — Project Context

## What This Is

A pure-Rust CM108/CM119 USB audio driver daemon (`cm108d`) for a ham radio repeater
controller. It replaces the kernel's `snd_usb_audio` and `hid` drivers with a direct
userspace driver that owns the device exclusively. Deployed on a Raspberry Pi 5
(aarch64, Debian 13 trixie) running the Repeater-Builder RB_RIM_Lite_v2 board.

**GitHub:** https://github.com/dfrederick15/cm108-driver-rust  
**Pi address:** repeaterd@172.16.11.188  
**Binary on Pi:** `/usr/sbin/cm108d` (systemd service: `cm108d.service`, enabled)  
**Socket:** `/run/cm108d/cm108d.sock`

---

## Hardware

**Board:** Repeater-Builder RB_RIM_Lite_v2 (N3XCC design)  
**Chip:** C-Media CM119, USB VID:PID `0d8c:013a`

### CM119 USB Interface Layout

| Interface | Type            | Alt | Endpoint | Dir | Max Pkt | Use              |
|-----------|-----------------|-----|----------|-----|---------|------------------|
| 0         | Audio Control   | 0   | —        | —   | —       | rusb owns        |
| 1         | Audio Streaming | 1   | 0x01     | OUT | 200 B   | usbfs ISO TX     |
| 2         | Audio Streaming | 1   | 0x82     | IN  | 100 B   | usbfs ISO RX     |
| 3         | HID             | 0   | 0x87     | IN  | 4 B     | rusb interrupt   |

**Audio format: mono 48 kHz 16-bit LE throughout.**
- RX (EP 0x82): 96 bytes/ms = 48 mono samples. Daemon duplicates L=R into stereo AudioFrame.
- TX (EP 0x01): 96 bytes/ms = 48 mono samples. Daemon takes left channel from stereo AudioFrame.

### GPIO Mapping (RB_RIM_Lite_v2)

| GPIO | Pin index | Direction | Function                              |
|------|-----------|-----------|---------------------------------------|
| 1    | 0         | Output    | PTT — keys the transmitter (active high) |
| 2    | 1         | Input     | COS — carrier-operated squelch from RX   |
| 3    | 2         | Input     | CTCSS/COS composite input                |
| 4    | 3         | Output    | PC_OK heartbeat LED (inverted: GPIO4 high → LED on) |

GPIO direction register: `0x09` (bits 0 and 3 = outputs, bits 1 and 2 = inputs).

**Heartbeat:** GPIO4 toggles on every IPC message received from a connected client.
LED blinks with client activity; stays off when no client is connected.

---

## Architecture

```
cm108d (server daemon)
├── rusb owns: interface 0 (audio control) + interface 3 (HID/GPIO)
│   └── EP 0x87 — HID interrupt poll, 20ms timeout → Ok(None) on timeout (not an error)
└── usbfs owns: interface 1 (audio OUT) + interface 2 (audio IN)
    ├── /dev/bus/usb/003/002 — opened via raw USBDEVFS ioctls
    ├── EP 0x01 — 8 ISO OUT URBs, 96 bytes each (mono TX)
    └── EP 0x82 — 8 ISO IN  URBs, 100 bytes max (mono RX)

Threads (all spawned by server.rs):
  cm108-rx       — ISO IN reap loop → AudioFrame ring buffer
  cm108-tx       — AudioFrame ring buffer → ISO OUT, silence on underrun
  cm108-dispatch — ring consumer → seqlock shmem write → notify AUDIO_IN subscribers
  cm108-gpio     — HID interrupt reads → broadcast RadioEvent to GPIO_EVENTS subscribers
  accept loop    — Unix socket accept, one handler thread per client
```

---

## Wiring a Client Into Application Code

### Option A — Rust (recommended)

Add to `Cargo.toml`:
```toml
cm108-client = { path = "/path/to/cm108-driver/crates/cm108-client" }
cm108-types  = { path = "/path/to/cm108-driver/crates/cm108-types" }
```

**Full usage example:**
```rust
use cm108_client::Cm108Client;
use cm108_types::{StreamFlags, ServerMsg, RadioEvent};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Cm108Client::connect(Path::new("/run/cm108d/cm108d.sock"))?;

    // Subscribe to audio input and GPIO events.
    client.subscribe(StreamFlags::AUDIO_IN | StreamFlags::GPIO_EVENTS)?;

    loop {
        // Blocks until a message arrives (AudioReady or RadioEvent).
        match client.wait_msg()? {
            Some(ServerMsg::AudioReady { seq: _ }) => {
                // Read the frame from seqlock shmem (zero-copy path).
                let frame = client.read_audio_latest();
                // frame.0 is [i16; 96]: indices 0,2,4… = L, 1,3,5… = R (L==R, mono).
                // 48 stereo pairs = 48 mono samples at 48 kHz.
                process_audio(&frame);
            }
            Some(ServerMsg::RadioEvent(ev)) => match ev {
                RadioEvent::PttAssert    => println!("PTT down"),
                RadioEvent::PttDeassert  => println!("PTT up"),
                RadioEvent::CosActive    => println!("COS active"),
                RadioEvent::CosInactive  => println!("COS inactive"),
                RadioEvent::GpioChange(state) => {
                    // state.pin(0) = GPIO1, state.pin(1) = GPIO2, etc.
                    println!("GPIO changed: {:08b}", state.0);
                }
            },
            Some(_) => {}  // Stats, Pong — ignore or handle
            None => break, // Server disconnected
        }
    }
    Ok(())
}

// Key the transmitter.
fn ptt_on(client: &mut Cm108Client)  { let _ = client.set_ptt(true); }
fn ptt_off(client: &mut Cm108Client) { let _ = client.set_ptt(false); }

// Set any GPIO output (pin 0 = GPIO1, pin 3 = GPIO4).
fn set_gpio(client: &mut Cm108Client, pin: u8, high: bool) {
    let _ = client.set_gpio(pin, high);
}
```

**Non-blocking GPIO poll pattern (for event loops):**
```rust
// Returns immediately — None if no event is waiting.
if let Ok(Some(msg)) = client.poll_msg() {
    // handle msg
}
```

**Blocking audio read (waits for next AudioReady notification):**
```rust
let frame = client.read_audio_blocking()?;
```

**Stats:**
```rust
let (rx_xruns, tx_xruns, latency) = client.get_stats()?;
println!("RX xruns={rx_xruns} TX xruns={tx_xruns} dispatch p99={}µs", latency.p99_us);
```

### Option B — C / C++

Build the shared library:
```bash
cd cm108-driver
PKG_CONFIG_ALLOW_CROSS=1 ... cargo build --release --target aarch64-unknown-linux-gnu
# produces: target/aarch64-unknown-linux-gnu/release/libcm108client.so
```

Install on the Pi:
```bash
scp target/aarch64-unknown-linux-gnu/release/libcm108client.so repeaterd@172.16.11.188:/usr/lib/
scp include/cm108.h repeaterd@172.16.11.188:/usr/include/
```

**C usage example:**
```c
#include <cm108.h>
#include <stdio.h>

int main(void) {
    Cm108Client *client = cm108_connect("/run/cm108d/cm108d.sock");
    if (!client) { fprintf(stderr, "connect failed\n"); return 1; }

    // Subscribe: AUDIO_IN (0x01) | GPIO_EVENTS (0x04)
    cm108_subscribe(client, 0x05);

    // Key the transmitter.
    cm108_set_ptt(client, 1);   // assert PTT (GPIO1 high)
    cm108_set_ptt(client, 0);   // deassert

    // Read one audio frame (blocks until available).
    // buf: 48 stereo pairs = 96 int16_t values, interleaved L0,R0,L1,R1,...
    // L == R always (mono source duplicated).
    int16_t buf[96];
    int32_t n = cm108_read_audio(client, buf, 48);

    // Poll GPIO events (non-blocking).
    Cm108Event ev;
    while (cm108_poll_event(client, &ev) == 1) {
        // ev.event_type: 0=PttAssert, 1=PttDeassert, 2=CosActive,
        //                3=CosInactive, 4=GpioChange
        // ev.gpio_state: bitmask (only meaningful for event_type==4)
        printf("event %d gpio=0x%02x\n", ev.event_type, ev.gpio_state);
    }

    cm108_destroy(client);
    return 0;
}
```

Compile on the Pi:
```bash
gcc -o myapp myapp.c -lcm108client
```

### Subscribe Flags

| Bit | Flag          | Value | Effect                                              |
|-----|---------------|-------|-----------------------------------------------------|
| 0   | `AUDIO_IN`    | 0x01  | Receive `AudioReady` notifications; read via shmem  |
| 1   | `AUDIO_OUT`   | 0x02  | Reserved (TX shmem path not yet wired end-to-end)   |
| 2   | `GPIO_EVENTS` | 0x04  | Receive `RadioEvent` messages for PTT/COS changes   |

### AudioFrame Layout

`AudioFrame.0` is `[i16; 96]` — 48 stereo pairs, interleaved L/R.
For this hardware L == R (mono duplicated). Each pair = one 48 kHz sample tick = ~20.8 µs.
One full frame = 1 ms of audio.

---

## IPC Protocol (wire format, if implementing a client from scratch)

Socket: Unix domain at `/run/cm108d/cm108d.sock`.

**On connect:** server immediately sends SCM_RIGHTS ancillary data containing the RX shmem
memfd. Receive it with `recvmsg`. The shmem is 4096 bytes, layout:
- bytes 0–7: `u64` seqlock counter (even = stable, odd = write in progress)
- bytes 8–199: `AudioFrame` payload (192 bytes, stereo i16 LE)

**Framing:** 4-byte little-endian length prefix + payload bytes.

**ClientMsg tags (1-byte discriminant):**
```
0x00  Subscribe  { streams: u8 }          — subscribe flags bitmask
0x01  SetGpio    { pin: u8, high: u8 }    — set GPIO output (pin 0–3)
0x02  AudioWrite { frame_count: u32 }     — (reserved, TX path not wired)
0x03  Ping                                — expect Pong response
0x04  GetStats                            — expect Stats response
```

**ServerMsg tags (1-byte discriminant):**
```
0x00  AudioReady { seq: u64 }             — new frame available at shmem seq
0x01  RadioEvent { tag: u8, state: u8 }   — GPIO event (tags: 0–4 as above)
0x02  Stats      { rx_xruns: u64, tx_xruns: u64, p50_us: u32, p99_us: u32, max_us: u32 }
0x03  Pong
0x04  Error      { len: u16, msg: utf8 }
```

All integers little-endian. See `crates/cm108-types/src/codec.rs` for the full encoder.

---

## Crate Structure

```
cm108-driver/
├── crates/
│   ├── cm108-types/     — shared types + hand-rolled codec (no serde)
│   │   └── src/
│   │       ├── lib.rs   — StreamFlags, AudioFrame, RadioEvent, GpioState, LatencyStats
│   │       └── codec.rs — Encode/Decode traits, wire format for all message types
│   ├── cm108-hal/       — hardware abstraction (rusb + raw usbfs ioctls)
│   │   └── src/
│   │       ├── lib.rs        — logger macros, HalError, set_log_level()
│   │       ├── device.rs     — Cm108Device::open() — detach ALSA, claim ctrl+HID ifaces
│   │       ├── hid_gpio.rs   — HidGpio::read_state() → Option<GpioState>
│   │       ├── iso_stream.rs — IsoStream: 8-URB ISO RX/TX via usbfs, mono conversion
│   │       ├── usbfs.rs      — USBDEVFS_SUBMITURB / REAPURBNDELAY ioctl wrappers
│   │       └── rt.rs         — SCHED_FIFO + CPU affinity + mlockall
│   ├── cm108-server/    — cm108d daemon binary
│   │   └── src/
│   │       ├── main.rs    — arg parse (no clap), log init, calls run()
│   │       ├── server.rs  — thread spawning, Unix socket accept loop
│   │       ├── ipc.rs     — per-client handler, ClientRegistry, heartbeat GPIO toggle
│   │       ├── latency.rs — latency histogram (lock-free u32 buckets)
│   │       └── shmem.rs   — seqlock AudioShmem via memfd + SCM_RIGHTS
│   └── cm108-client/    — Rust client library + C FFI (libcm108client)
│       └── src/
│           ├── lib.rs      — Cm108Client, ClientError, re-exports
│           ├── client.rs   — connect, subscribe, read_audio, set_ptt, poll_msg, get_stats
│           ├── ffi.rs      — #[no_mangle] C API: cm108_connect/destroy/subscribe/etc.
│           └── framing.rs  — write_client_msg / read_server_msg using codec
├── include/cm108.h      — C header (hand-written, maintained manually)
├── udev/90-cm108.rules  — installed at /etc/udev/rules.d/ on Pi; prevents ALSA rebind
├── systemd/cm108d.service — enabled on Pi; Type=simple, Restart=on-failure
└── CONTEXT.md           — this file
```

---

## Dependencies (intentionally minimal)

| Crate  | Why kept                                                    |
|--------|-------------------------------------------------------------|
| `rusb` | USB enumeration, interface claim, HID interrupt transfers   |
| `rtrb` | Lock-free SPSC ring buffer (correctness-critical on ARM)    |
| `libc` | POSIX types, USBDEVFS ioctls, mmap, memfd, SCM_RIGHTS       |

Eliminated: `anyhow`, `thiserror`, `bitflags`, `nix`, `clap`, `tracing`,
`tracing-subscriber`, `postcard`, `serde`, `cbindgen`.

---

## Cross-Compile and Deploy

```bash
# On dev machine (x86_64 Debian):
PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig \
    ~/.cargo/bin/cargo build --release --target aarch64-unknown-linux-gnu

scp target/aarch64-unknown-linux-gnu/release/cm108d repeaterd@172.16.11.188:/tmp/cm108d
ssh repeaterd@172.16.11.188 '
    sudo systemctl stop cm108d
    sudo cp /tmp/cm108d /usr/sbin/cm108d
    sudo systemctl start cm108d
'
```

Run options:
```bash
sudo cm108d --socket /run/cm108d/cm108d.sock --log debug
# or via env: CM108_SOCKET=/run/cm108d/cm108d.sock CM108_LOG=info sudo cm108d
```

Log levels: `trace`, `debug`, `info`, `warn`, `error`.

On reboot: `snd_usb_audio` reclaims the CM119, but `cm108d` detaches all 4 interfaces
at startup via `libusb_detach_kernel_driver`. The udev rule prevents rebinding on hotplug.

---

## Current Status

| Feature                        | Status  | Notes                                            |
|--------------------------------|---------|--------------------------------------------------|
| USB open + ALSA detach         | ✅ Done  | All 4 interfaces detached at startup             |
| HID GPIO polling (PTT/COS)     | ✅ Done  | EP 0x87, 20ms timeout, silence = no event        |
| GPIO output (PTT key)          | ✅ Done  | set_ptt / set_gpio via HID control report        |
| Heartbeat LED (GPIO4/PC_OK)    | ✅ Done  | Toggles on each IPC message from client          |
| ISO RX audio (from radio)      | ✅ Done  | 8 URBs, mono 96 B/ms, ~10% CPU                  |
| ISO TX audio (to radio)        | ✅ Done  | 8 URBs, mono 96 B/ms, silence on underrun        |
| Shmem audio delivery           | ✅ Done  | seqlock memfd, fd via SCM_RIGHTS on connect      |
| C FFI client library           | ✅ Done  | libcm108client.so + include/cm108.h              |
| Systemd service                | ✅ Done  | Enabled, auto-restarts on failure                |
| TX write_audio path            | ⬜ Open  | cm108_write_audio() is a no-op; TX ring not wired to client |
| ISO audio format validation    | ⬜ Open  | Needs real audio flowing through the board       |
| `rusb` replacement             | ⬜ Defer | Replace with raw USBDEVFS; blocked on validation |
| `rtrb` replacement             | ⬜ Defer | Hand-roll lock-free ring; deferred               |
