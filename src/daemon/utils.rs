use std::time::Duration;

use tokio::{process::Child, time::timeout};
use tracing::{error, warn};

pub async fn kill_child(mut child: Child, context: String, grace_time_sec: u64) {
    const STREAM_PROCESS_KILL_WAIT_TIMEOUT_SEC: u64 = 5;

    // first, try to ask nicely the child process to exit
    if let Some(pid) = child.id() {
        let pid = nix::unistd::Pid::from_raw(pid as i32);

        // the most likely error is that the process does not exist anymore
        // but we still need to reclaim it with Child::wait
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::SIGTERM);

        if let Ok(_) = timeout(Duration::from_secs(grace_time_sec), child.wait()).await {
            return;
        }
    } else {
        warn!("could not get PID of child process for {context}");
        // still try to use tokio API to kill and reclaim the child process
    }

    // if that fails, or we cannot get the underlying PID, terminate the process.
    // NOTE: processes killed with SIGKILL are not guaranteed to exit. They can be locked up in a
    // syscall to a resource no-longer available (a notorious example is a read on a disconnected
    // NFS share)

    // as before, the only expected error is that the child process already terminated
    // but we still need to reclaim it if that's the case.
    warn!("process for {context} didn't exit {grace_time_sec}s after SIGTERM, sending SIGKILL");
    let _ = child.start_kill();

    match timeout(
        Duration::from_secs(STREAM_PROCESS_KILL_WAIT_TIMEOUT_SEC),
        child.wait(),
    )
    .await
    {
        Ok(_) => {}
        Err(_) => match child.id() {
            Some(id) => {
                error!("child process of {context} did not terminate. PID: {id}");
            }
            None => {
                error!("child process of {context} did not terminate");
            }
        },
    }
}
