use std::io::{self, Read, Write};

use cm108_types::{ClientMsg, ServerMsg};

pub fn write_client_msg(stream: &mut impl Write, msg: &ClientMsg) -> io::Result<()> {
    let payload = postcard::to_allocvec(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    stream.write_all(&(payload.len() as u32).to_le_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()
}

pub fn read_server_msg(stream: &mut impl Read) -> io::Result<Option<ServerMsg>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Err(e)
            if e.kind() == io::ErrorKind::UnexpectedEof
                || e.kind() == io::ErrorKind::WouldBlock =>
        {
            return Ok(None)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use cm108_types::{ClientMsg, LatencyStats, RadioEvent, ServerMsg, StreamFlags};
    use std::io::Cursor;

    fn write_server_msg(buf: &mut impl Write, msg: &ServerMsg) -> io::Result<()> {
        let payload = postcard::to_allocvec(msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        buf.write_all(&(payload.len() as u32).to_le_bytes())?;
        buf.write_all(&payload)
    }

    #[test]
    fn client_msg_framing_roundtrip() {
        let msgs = [
            ClientMsg::Ping,
            ClientMsg::GetStats,
            ClientMsg::Subscribe { streams: StreamFlags::AUDIO_IN | StreamFlags::GPIO_EVENTS },
            ClientMsg::SetGpio { pin: 2, high: false },
            ClientMsg::AudioWrite { frame_count: 100 },
        ];
        for msg in &msgs {
            let mut buf = Vec::new();
            write_client_msg(&mut buf, msg).unwrap();

            // Verify length prefix
            let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
            assert_eq!(buf.len(), 4 + len);

            // Decode payload
            let decoded: ClientMsg = postcard::from_bytes(&buf[4..]).unwrap();
            let re = postcard::to_allocvec(&decoded).unwrap();
            assert_eq!(re, buf[4..], "framing mismatch for {msg:?}");
        }
    }

    #[test]
    fn server_msg_framing_roundtrip() {
        let msgs = [
            ServerMsg::Pong,
            ServerMsg::AudioReady { seq: 12345 },
            ServerMsg::RadioEvent(RadioEvent::PttAssert),
            ServerMsg::Stats {
                rx_xruns: 0,
                tx_xruns: 1,
                dispatch_lat: LatencyStats { min_us: 2, max_us: 50, p99_us: 32 },
            },
            ServerMsg::Error("oops".into()),
        ];
        for msg in &msgs {
            let mut buf = Vec::new();
            write_server_msg(&mut buf, msg).unwrap();
            let decoded = read_server_msg(&mut Cursor::new(&buf)).unwrap().unwrap();
            assert_eq!(&decoded, msg, "framing mismatch for {msg:?}");
        }
    }

    #[test]
    fn eof_returns_none() {
        let buf: &[u8] = &[];
        let result = read_server_msg(&mut Cursor::new(buf)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn oversized_message_is_rejected() {
        let mut buf = Vec::new();
        // Write a length header claiming 128 KiB
        buf.extend_from_slice(&128u32.saturating_mul(1024).to_le_bytes());
        let result = read_server_msg(&mut Cursor::new(&buf));
        assert!(result.is_err());
    }
}
