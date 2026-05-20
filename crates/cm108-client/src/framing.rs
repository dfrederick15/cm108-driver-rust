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
