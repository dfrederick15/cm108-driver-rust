use std::os::unix::net::UnixStream;
use std::path::Path;

use cm108_types::{ClientMsg, ServerMsg, StreamFlags};

use crate::Result;

pub struct Cm108Client {
    _stream: UnixStream,
}

impl Cm108Client {
    pub fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        Ok(Self { _stream: stream })
    }

    pub fn subscribe(&mut self, flags: StreamFlags) -> Result<()> {
        let _msg = ClientMsg::Subscribe { streams: flags };
        // TODO: Phase 4 — encode with postcard, write to socket
        Ok(())
    }

    pub fn set_ptt(&mut self, asserted: bool) -> Result<()> {
        let _msg = ClientMsg::SetGpio { pin: 0, high: asserted };
        // TODO: Phase 4
        Ok(())
    }

    pub fn set_gpio(&mut self, pin: u8, high: bool) -> Result<()> {
        let _msg = ClientMsg::SetGpio { pin, high };
        // TODO: Phase 4
        Ok(())
    }

    pub fn poll_server_msg(&mut self) -> Result<Option<ServerMsg>> {
        // TODO: Phase 4 — non-blocking read + postcard decode
        Ok(None)
    }
}
