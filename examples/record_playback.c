/*
 * record_playback.c — records 5 seconds of audio from cm108d, writes a
 * minimal WAV file, then plays it back through the same device.
 *
 * Build (after `cargo build --release --features generate-header`):
 *   gcc -O2 -o record_playback examples/record_playback.c \
 *       -Iinclude -Ltarget/release -lcm108client -lpthread
 *
 * Run:
 *   ./record_playback /run/cm108d/cm108d.sock output.wav
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "cm108.h"

/* ── Minimal WAV writer ──────────────────────────────────────────────────── */

#define SAMPLE_RATE  48000
#define CHANNELS     2
#define BITS         16
#define RECORD_SECS  5

static void write_u32_le(FILE *f, uint32_t v) {
    uint8_t b[4] = { v, v >> 8, v >> 16, v >> 24 };
    fwrite(b, 1, 4, f);
}

static void write_u16_le(FILE *f, uint16_t v) {
    uint8_t b[2] = { v, v >> 8 };
    fwrite(b, 1, 2, f);
}

static FILE *open_wav(const char *path, uint32_t total_samples) {
    FILE *f = fopen(path, "wb");
    if (!f) return NULL;

    uint32_t data_bytes = total_samples * CHANNELS * (BITS / 8);
    uint32_t byte_rate  = SAMPLE_RATE * CHANNELS * (BITS / 8);
    uint16_t block_align = CHANNELS * (BITS / 8);

    /* RIFF header */
    fwrite("RIFF", 1, 4, f);
    write_u32_le(f, 36 + data_bytes);   /* ChunkSize */
    fwrite("WAVE", 1, 4, f);
    /* fmt  sub-chunk */
    fwrite("fmt ", 1, 4, f);
    write_u32_le(f, 16);                /* Subchunk1Size (PCM) */
    write_u16_le(f, 1);                 /* AudioFormat = PCM */
    write_u16_le(f, CHANNELS);
    write_u32_le(f, SAMPLE_RATE);
    write_u32_le(f, byte_rate);
    write_u16_le(f, block_align);
    write_u16_le(f, BITS);
    /* data sub-chunk header */
    fwrite("data", 1, 4, f);
    write_u32_le(f, data_bytes);
    return f;
}

/* ── Main ────────────────────────────────────────────────────────────────── */

int main(int argc, char **argv) {
    const char *sock  = argc > 1 ? argv[1] : "/run/cm108d/cm108d.sock";
    const char *outf  = argc > 2 ? argv[2] : "output.wav";

    /* frames_per_call = 48 stereo frames per AudioFrame (1 ms @ 48 kHz) */
    const int FPC   = 48;
    const int TOTAL = SAMPLE_RATE * RECORD_SECS; /* total mono-channel samples */
    const int CALLS = TOTAL / FPC;

    int16_t *pcm = malloc(CALLS * FPC * CHANNELS * sizeof(int16_t));
    if (!pcm) { fputs("out of memory\n", stderr); return 1; }

    /* ── Connect ── */
    cm108_client_t *client = cm108_connect(sock);
    if (!client) {
        fprintf(stderr, "cm108_connect(%s) failed\n", sock);
        free(pcm);
        return 1;
    }

    /* Subscribe to audio input and GPIO events */
    cm108_subscribe(client, 0x05); /* AUDIO_IN=1 | GPIO_EVENTS=4 */

    /* ── Record ── */
    printf("Recording %d seconds…\n", RECORD_SECS);
    for (int i = 0; i < CALLS; i++) {
        int n = cm108_read_audio(client, pcm + i * FPC * CHANNELS, FPC);
        if (n < 0) {
            fputs("cm108_read_audio failed\n", stderr);
            cm108_destroy(client);
            free(pcm);
            return 1;
        }

        /* Drain any queued events without blocking */
        Cm108Event ev;
        while (cm108_poll_event(client, &ev) == 1) {
            const char *names[] = {
                "PTT-assert", "PTT-deassert",
                "COS-active", "COS-inactive", "GPIO-change"
            };
            printf("  event: %s (gpio=0x%02x)\n",
                   ev.event_type < 5 ? names[ev.event_type] : "unknown",
                   ev.gpio_state);
        }
    }

    /* Write WAV */
    FILE *wav = open_wav(outf, (uint32_t)(CALLS * FPC));
    if (!wav) {
        perror(outf);
    } else {
        fwrite(pcm, sizeof(int16_t), CALLS * FPC * CHANNELS, wav);
        fclose(wav);
        printf("Saved %s\n", outf);
    }

    /* ── Assert PTT and play back ── */
    printf("Playing back…\n");
    cm108_set_ptt(client, 1);
    for (int i = 0; i < CALLS; i++) {
        cm108_write_audio(client, pcm + i * FPC * CHANNELS, FPC);
    }
    cm108_set_ptt(client, 0);
    printf("Done.\n");

    cm108_destroy(client);
    free(pcm);
    return 0;
}
