use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::Arc;

use cm108_hal::{Cm108Device, IsoStream};
use tracing::info;

pub fn run(socket_path: &str) -> anyhow::Result<()> {
    let device = Arc::new(Cm108Device::open()?);

    let stream = IsoStream::start(
        Arc::clone(&device),
        90, // rx priority
        90, // tx priority
        1,  // rx core
        1,  // tx core
    )?;

    let sock_path = Path::new(socket_path);
    if sock_path.exists() {
        std::fs::remove_file(sock_path)?;
    }
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(sock_path)?;
    info!(socket = socket_path, "cm108d listening");

    for conn in listener.incoming() {
        match conn {
            Ok(stream_conn) => {
                let _ = stream_conn; // TODO: Phase 3 — IPC client handler
            }
            Err(e) => tracing::warn!("accept error: {e}"),
        }
    }

    drop(stream);
    Ok(())
}
