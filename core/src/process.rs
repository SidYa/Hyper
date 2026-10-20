//! Process control shared by the game runner and the Proton wrapper: a
//! handle that terminates a process tree from anywhere, with system calls
//! only (no `kill` / `taskkill` child processes).
//!
//! A PID is recycled once its process is gone, so a handle remembers the
//! process's start time as well and kills nothing whose start time differs:
//! a game that exited an hour ago must not take an unrelated program with
//! the same number down.

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Handle that can terminate a spawned process (and its children) from
/// anywhere, without holding the `Child` itself.
#[derive(Debug, Clone)]
pub struct Killer {
    pid: u32,
    /// Seconds since the epoch when the process started (sysinfo's
    /// `start_time`); `None` when the process was gone already when the
    /// handle was made, in which case there is nothing to kill.
    start_time: Option<u64>,
}

/// Start times of the same process read at different moments may differ by
/// rounding (Linux: boot time + clock ticks).
const START_TOLERANCE: u64 = 1;

impl Killer {
    /// A handle for a process that runs right now: its start time is looked
    /// up, so a later kill reaches only this process (or its tree), never a
    /// newcomer that got the same PID.
    pub fn from_pid(pid: u32) -> Self {
        if pid <= 1 {
            return Self {
                pid,
                start_time: None,
            };
        }
        let start_time = start_time_of(pid);
        if start_time.is_none() {
            tracing::debug!(
                pid,
                "process not found when creating its handle; nothing to kill later"
            );
        }
        Self { pid, start_time }
    }

    /// A handle from a process listing that already knows the start time.
    pub fn with_start_time(pid: u32, start_time: u64) -> Self {
        Self {
            pid,
            start_time: (pid > 1).then_some(start_time),
        }
    }

    /// Terminate the whole process tree. Best effort; a no-op when the
    /// process is gone (its PID may belong to someone else by now).
    pub async fn kill(&self) {
        let (Some(start), true) = (self.start_time, self.pid > 1) else {
            return;
        };
        let pid = self.pid;
        let same = tokio::task::spawn_blocking(move || same_process(pid, start))
            .await
            .unwrap_or(false);
        if !same {
            tracing::info!(pid, "process gone (or its PID reused): nothing to kill");
            return;
        }
        tracing::info!(pid, "killing process tree");
        #[cfg(windows)]
        {
            let _ = tokio::task::spawn_blocking(move || kill_tree(pid, start)).await;
        }
        #[cfg(unix)]
        {
            let Ok(pid) = i32::try_from(pid) else { return };
            // games run in their own process group (runtime → Proton → Wine,
            // see `GameRunner::launch`): signal all of it, then the process
            // itself (a lone process is no group leader, its group signal
            // just fails)
            let is_group = signal(-pid, libc::SIGTERM);
            signal(pid, libc::SIGTERM);
            // Wine may ignore TERM: whatever of the group is left gets KILL
            if is_group {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                if signal(-pid, 0) {
                    signal(-pid, libc::SIGKILL);
                }
            }
        }
    }
}

/// Start time of a live process, `None` when there is none with that PID.
fn start_time_of(pid: u32) -> Option<u64> {
    let mut sys = System::new();
    let pid = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(pid).map(|p| p.start_time())
}

fn same_start(a: u64, b: u64) -> bool {
    a.abs_diff(b) <= START_TOLERANCE
}

/// Whether `pid` still is the process that started at `start`.
fn same_process(pid: u32, start: u64) -> bool {
    start_time_of(pid).is_some_and(|now| same_start(now, start))
}

/// `kill(2)`; true when the target exists and the signal was delivered.
/// Never signals PID 0 / 1 or "every process" (`-1`).
#[cfg(unix)]
fn signal(target: i32, sig: i32) -> bool {
    if target.abs() <= 1 {
        return false;
    }
    // SAFETY: plain system call with integer arguments
    unsafe { libc::kill(target, sig) == 0 }
}

/// Windows has no process groups to signal: walk the parent links of a
/// process snapshot and terminate the root with every descendant, children
/// first (what `taskkill /T /F` did). The root must still be the process
/// that started at `root_start`, and a child counts only when it is younger
/// than its parent (a parent PID is recycled too, and Windows keeps the
/// stale link).
#[cfg(windows)]
fn kill_tree(root: u32, root_start: u64) {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let root = Pid::from_u32(root);
    let Some(root_now) = sys.process(root) else {
        tracing::debug!(pid = root.as_u32(), "process already gone");
        return;
    };
    if !same_start(root_now.start_time(), root_start) {
        tracing::info!(
            pid = root.as_u32(),
            "PID reused by another process; not killing it"
        );
        return;
    }
    let mut targets = vec![root];
    let mut i = 0;
    while i < targets.len() {
        let parent = targets[i];
        i += 1;
        let parent_start = sys.process(parent).map(|p| p.start_time()).unwrap_or(0);
        for (pid, process) in sys.processes() {
            if process.parent() == Some(parent)
                && !targets.contains(pid)
                && process.start_time() >= parent_start
            {
                targets.push(*pid);
            }
        }
    }
    for pid in targets.iter().rev() {
        match sys.process(*pid) {
            Some(process) => {
                if !process.kill() {
                    tracing::warn!(pid = pid.as_u32(), "could not terminate process");
                }
            }
            None => tracing::debug!(pid = pid.as_u32(), "process already gone"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn kills_a_process_and_its_child() {
        // a shell that starts a sleeping child; killing the tree ends both
        #[cfg(windows)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
            c
        };
        #[cfg(unix)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "sleep 30"]);
            c.process_group(0);
            c
        };
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().expect("spawn");
        let pid = child.id().expect("pid");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let killer = Killer::from_pid(pid);
        assert!(
            killer.start_time.is_some(),
            "a running process has a start time"
        );
        killer.kill().await;
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .expect("exits in time")
            .expect("wait");
        assert!(!status.success());
    }

    #[tokio::test]
    async fn a_reused_pid_is_not_killed() {
        // the same PID with another start time is another process
        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "sleep 30"]);
            c
        };
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().expect("spawn");
        let pid = child.id().expect("pid");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let real = start_time_of(pid).expect("running");
        Killer::with_start_time(pid, real.saturating_sub(3600))
            .kill()
            .await;
        assert!(
            same_process(pid, real),
            "a stale handle must leave the process alone"
        );
        child.kill().await.expect("kill");
        let _ = child.wait().await;
    }

    #[test]
    fn pid_zero_and_one_are_ignored() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        for pid in [0, 1] {
            let k = Killer::from_pid(pid);
            assert!(k.start_time.is_none());
            rt.block_on(k.kill());
            assert!(Killer::with_start_time(pid, 5).start_time.is_none());
        }
    }
}
