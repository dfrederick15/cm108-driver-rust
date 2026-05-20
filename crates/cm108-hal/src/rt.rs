use libc::{cpu_set_t, sched_param, CPU_SET, CPU_ZERO};
use tracing::warn;

/// Apply SCHED_FIFO scheduling and CPU affinity to the calling thread.
/// Logs a warning (does not panic) if privileges are insufficient.
pub fn configure_rt(priority: i32, core: usize) {
    set_sched_fifo(priority);
    set_affinity(core);
    mlockall();
}

fn set_sched_fifo(priority: i32) {
    let param = sched_param { sched_priority: priority };
    let ret = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if ret != 0 {
        warn!(
            priority,
            errno = unsafe { *libc::__errno_location() },
            "sched_setscheduler failed — running without RT priority"
        );
    }
}

fn set_affinity(core: usize) {
    let mut set = unsafe {
        let mut s: cpu_set_t = std::mem::zeroed();
        CPU_ZERO(&mut s);
        CPU_SET(core, &mut s);
        s
    };
    let ret = unsafe {
        libc::sched_setaffinity(0, std::mem::size_of::<cpu_set_t>(), &mut set)
    };
    if ret != 0 {
        warn!(core, "sched_setaffinity failed");
    }
}

fn mlockall() {
    let ret = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
    if ret != 0 {
        warn!("mlockall failed — memory may be paged out under pressure");
    }
}
