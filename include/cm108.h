/**
 * cm108.h — Public C API for libcm108client
 *
 * Links against libcm108client (built from crates/cm108-client).
 * Thread-safety: all functions are safe to call from a single thread.
 * Concurrent calls on the same cm108_client_t from multiple threads require
 * external synchronisation.
 */

#ifndef CM108_H
#define CM108_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque client handle. */
typedef struct Cm108Client Cm108Client;

/**
 * C-compatible radio / GPIO event.
 *
 * event_type values:
 *   0  PttAssert      — PTT key-down detected on GPIO1
 *   1  PttDeassert    — PTT key-up detected on GPIO1
 *   2  CosActive      — carrier detected on GPIO2
 *   3  CosInactive    — carrier dropped on GPIO2
 *   4  GpioChange     — any other GPIO state change; see gpio_state
 *
 * gpio_state:  bitmask of GPIO1-GPIO4 current levels (bit 0 = GPIO1).
 *              Only meaningful for event_type == 4.
 */
typedef struct Cm108Event {
    uint8_t event_type;
    uint8_t gpio_state;
} Cm108Event;

/* ── Lifecycle ───────────────────────────────────────────────────────────── */

/**
 * Connect to a running cm108d server at socket_path.
 * Returns an opaque handle on success, or NULL on failure.
 * The handle must be freed with cm108_destroy() when no longer needed.
 */
Cm108Client *cm108_connect(const char *socket_path);

/**
 * Close the connection and free all resources associated with client.
 * Passing NULL is a no-op.
 */
void cm108_destroy(Cm108Client *client);

/* ── Subscriptions ───────────────────────────────────────────────────────── */

/**
 * Subscribe to event/audio streams.
 *
 * flags bitmask:
 *   0x01  AUDIO_IN    — receive audio input frames via cm108_read_audio()
 *   0x02  AUDIO_OUT   — send audio output frames via cm108_write_audio()
 *   0x04  GPIO_EVENTS — receive GPIO/PTT/COS events via cm108_poll_event()
 *
 * Returns 0 on success, -1 on error.
 */
int32_t cm108_subscribe(Cm108Client *client, uint8_t flags);

/* ── Audio I/O ───────────────────────────────────────────────────────────── */

/**
 * Block until the next audio input frame is available, then copy up to
 * `frames` stereo samples into buf.
 *
 * buf must hold at least frames * 2 * sizeof(int16_t) bytes.
 * Samples are interleaved: [L0, R0, L1, R1, …].
 *
 * Returns the number of stereo frames copied (≤ frames), or -1 on error.
 */
int32_t cm108_read_audio(Cm108Client *client, int16_t *buf, size_t frames);

/**
 * Write frames stereo samples from buf for TX playback.
 *
 * Returns frames on success (no-op until TX shmem path is implemented),
 * or -1 on error.
 */
int32_t cm108_write_audio(Cm108Client *client, const int16_t *buf, size_t frames);

/* ── GPIO control ────────────────────────────────────────────────────────── */

/**
 * Assert (asserted != 0) or deassert PTT on GPIO1.
 * Returns 0 on success, -1 on error.
 */
int32_t cm108_set_ptt(Cm108Client *client, int32_t asserted);

/**
 * Set an arbitrary GPIO pin.
 * pin: 0-indexed (0 = GPIO1 … 3 = GPIO4).
 * high: 1 = set high, 0 = set low.
 * Returns 0 on success, -1 on error.
 */
int32_t cm108_set_gpio(Cm108Client *client, uint8_t pin, int32_t high);

/* ── Event polling ───────────────────────────────────────────────────────── */

/**
 * Non-blocking poll for a pending server event.
 * Fills *out if an event is available.
 *
 * Returns:
 *   1  — event written to *out
 *   0  — no event pending
 *  -1  — error (client is NULL or I/O failure)
 */
int32_t cm108_poll_event(Cm108Client *client, Cm108Event *out);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* CM108_H */
