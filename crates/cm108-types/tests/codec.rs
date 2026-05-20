use cm108_types::*;

// ── Type invariants ───────────────────────────────────────────────────────────

#[test]
fn audio_frame_default_is_silence() {
    let f = AudioFrame::default();
    assert!(f.0.iter().all(|&s| s == 0));
}

#[test]
fn audio_frame_size_matches_frame_bytes() {
    assert_eq!(
        std::mem::size_of::<[i16; SAMPLES_PER_FRAME * 2]>(),
        FRAME_BYTES,
    );
}

#[test]
fn gpio_state_pin_bits() {
    let g = GpioState(0b1010);
    assert!(!g.pin(0)); // GPIO1 clear
    assert!(g.pin(1));  // GPIO2 set
    assert!(!g.pin(2)); // GPIO3 clear
    assert!(g.pin(3));  // GPIO4 set
}

#[test]
fn stream_flags_combinations() {
    let f = StreamFlags::AUDIO_IN | StreamFlags::GPIO_EVENTS;
    assert!(f.contains(StreamFlags::AUDIO_IN));
    assert!(!f.contains(StreamFlags::AUDIO_OUT));
    assert!(f.contains(StreamFlags::GPIO_EVENTS));
    assert_eq!(f.bits(), 0b0101);
}

#[test]
fn latency_stats_default_is_zero() {
    let ls = LatencyStats::default();
    assert_eq!(ls.min_us, 0);
    assert_eq!(ls.max_us, 0);
    assert_eq!(ls.p99_us, 0);
}

// ── Postcard round-trips ──────────────────────────────────────────────────────

fn rt<T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug>(
    msg: T,
) {
    let encoded = postcard::to_allocvec(&msg).unwrap();
    let decoded: T = postcard::from_bytes(&encoded).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn client_msg_roundtrips() {
    rt(ClientMsg::Ping);
    rt(ClientMsg::GetStats);
    rt(ClientMsg::Subscribe {
        streams: StreamFlags::AUDIO_IN | StreamFlags::GPIO_EVENTS,
    });
    rt(ClientMsg::SetGpio { pin: 0, high: true });
    rt(ClientMsg::SetGpio { pin: 3, high: false });
    rt(ClientMsg::AudioWrite { frame_count: 42 });
}

#[test]
fn server_msg_roundtrips() {
    rt(ServerMsg::Pong);
    rt(ServerMsg::AudioReady { seq: u64::MAX });
    rt(ServerMsg::RadioEvent(RadioEvent::PttAssert));
    rt(ServerMsg::RadioEvent(RadioEvent::CosInactive));
    rt(ServerMsg::RadioEvent(RadioEvent::GpioChange(GpioState(0b1111))));
    rt(ServerMsg::Stats {
        rx_xruns: 7,
        tx_xruns: 3,
        dispatch_lat: LatencyStats { min_us: 1, max_us: 500, p99_us: 120 },
    });
    rt(ServerMsg::Error("test error".into()));
}

#[test]
fn stream_flags_roundtrip() {
    rt(StreamFlags::AUDIO_IN | StreamFlags::AUDIO_OUT | StreamFlags::GPIO_EVENTS);
    rt(StreamFlags::empty());
}

#[test]
fn radio_event_roundtrip_all_variants() {
    for ev in [
        RadioEvent::PttAssert,
        RadioEvent::PttDeassert,
        RadioEvent::CosActive,
        RadioEvent::CosInactive,
        RadioEvent::GpioChange(GpioState(0xAB)),
    ] {
        rt(ev);
    }
}
