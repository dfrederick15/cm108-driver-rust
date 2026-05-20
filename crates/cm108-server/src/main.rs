mod ipc;
mod server;
mod shmem;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "cm108d", about = "CM108/CM119 real-time audio server")]
struct Args {
    /// Unix socket path
    #[arg(long, default_value = "/run/cm108d/cm108d.sock")]
    socket: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info", env = "CM108_LOG")]
    log: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&args.log))
        .init();

    server::run(&args.socket)
}
