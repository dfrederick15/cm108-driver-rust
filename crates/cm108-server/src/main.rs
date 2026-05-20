use std::sync::atomic::{AtomicU8, Ordering};

// ── Minimal logger ────────────────────────────────────────────────────────────
// Level encoding: 0=trace, 1=debug, 2=info, 3=warn, 4=error

static LOG_LEVEL: AtomicU8 = AtomicU8::new(2); // default: info

macro_rules! log_info {
    ($($t:tt)*) => {
        if crate::LOG_LEVEL.load(::std::sync::atomic::Ordering::Relaxed) <= 2 {
            ::std::eprintln!("[INFO]  {}", ::std::format_args!($($t)*));
        }
    };
}
macro_rules! log_warn {
    ($($t:tt)*) => {
        if crate::LOG_LEVEL.load(::std::sync::atomic::Ordering::Relaxed) <= 3 {
            ::std::eprintln!("[WARN]  {}", ::std::format_args!($($t)*));
        }
    };
}
macro_rules! log_debug {
    ($($t:tt)*) => {
        if crate::LOG_LEVEL.load(::std::sync::atomic::Ordering::Relaxed) <= 1 {
            ::std::eprintln!("[DEBUG] {}", ::std::format_args!($($t)*));
        }
    };
}

mod ipc;
mod latency;
mod server;
mod shmem;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (socket, log_str) = parse_args();

    let level = level_from_str(&log_str);
    LOG_LEVEL.store(level, Ordering::Relaxed);
    cm108_hal::set_log_level(level);

    server::run(&socket)
}

// ── Argument parsing ──────────────────────────────────────────────────────────

fn parse_args() -> (String, String) {
    let mut socket =
        std::env::var("CM108_SOCKET").unwrap_or_else(|_| "/run/cm108d/cm108d.sock".into());
    let mut log_level =
        std::env::var("CM108_LOG").unwrap_or_else(|_| "info".into());

    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--help" | "-h" => {
                eprintln!("Usage: cm108d [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --socket PATH   Unix socket path  (env: CM108_SOCKET, default: /run/cm108d/cm108d.sock)");
                eprintln!("  --log LEVEL     Log level         (env: CM108_LOG,    default: info)");
                eprintln!("                  Levels: trace | debug | info | warn | error");
                eprintln!("  --help          Show this help");
                std::process::exit(0);
            }
            arg if arg.starts_with("--socket=") => {
                socket = arg["--socket=".len()..].to_string();
            }
            "--socket" => {
                i += 1;
                socket = raw.get(i).cloned().unwrap_or_default();
            }
            arg if arg.starts_with("--log=") => {
                log_level = arg["--log=".len()..].to_string();
            }
            "--log" => {
                i += 1;
                log_level = raw.get(i).cloned().unwrap_or_else(|| "info".into());
            }
            arg => {
                eprintln!("cm108d: unknown argument: {arg}");
                std::process::exit(1);
            }
        }
        i += 1;
    }
    (socket, log_level)
}

fn level_from_str(s: &str) -> u8 {
    match s {
        "trace" => 0,
        "debug" => 1,
        "info"  => 2,
        "warn"  => 3,
        "error" => 4,
        _       => 2,
    }
}
