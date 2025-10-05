use std::time::Duration;

use futures::FutureExt;
use tokio::{process::Child, time::sleep};
use tracing::{error, warn};

pub async fn kill_child(mut child: Child, context: String, grace_time_sec: u64) {
    const STREAM_PROCESS_KILL_WAIT_TIMEOUT_SEC: u64 = 5;

    // first, try to ask nicely the child process to exit
    if let Some(pid) = child.id() {
        let pid = nix::unistd::Pid::from_raw(pid as i32);

        // the most likely error is that the process does not exist anymore
        // but we still need to reclaim it with Child::wait
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::SIGTERM);

        futures::select! {
            _ = child.wait().fuse() => {
                return;
            },
            _ = sleep(Duration::from_secs(grace_time_sec)).fuse() => {},
        }
    } else {
        warn!("could not get PID of child process for {}", context);
        // still try to use tokio API to kill and reclaim the child process
    }

    // if that fails, or we cannot get the underlying PID, terminate the process.
    // NOTE: processes killed with SIGKILL are not guaranteed to exit. They can be locked up in a
    // syscall to a resource no-longer available (a notorious example is a read on a disconnected
    // NFS share)

    // as before, the only expected error is that the child process already terminated
    // but we still need to reclaim it if that's the case.
    let _ = child.start_kill();

    futures::select! {
        _ = child.wait().fuse() => {}
        _ = sleep(Duration::from_secs(STREAM_PROCESS_KILL_WAIT_TIMEOUT_SEC)).fuse() => {
            error!("child process of {} did not terminate", context);
        }
    }
}
