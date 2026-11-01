//! Process-group (tree) teardown for spawned children.
//!
//! `Child::start_kill` and `kill_on_drop` signal only the direct child. Every
//! real workload forks: `sh -c 'server &'` leaves the server running, an MCP
//! server leaves its language runtime. Killing the shell therefore leaked the
//! grandchildren, which kept running -- and kept the inherited stdout pipe open,
//! so the caller could not even observe EOF.
//!
//! A managed child is spawned as the leader of its own process group and torn
//! down as a group. This module stays synchronous on purpose: it needs only
//! `std::process` plus one OS call, so `ark-common` keeps its "no async runtime"
//! invariant and both `ark-tool` and `ark-mcp` reuse it instead of growing a new
//! cross-crate edge. Reaping the child (`wait`) stays with the caller, which is
//! the only place that knows whether it also owns the pipes.
//!
//! Platform split, confined to [`imp`]:
//!
//! * Unix -- [`isolate`] sets `process_group(0)` at spawn, so `pgid == pid` and
//!   `killpg(SIGKILL)` reaches every descendant. A `killpg` that fails because
//!   the group already exited (`ESRCH`) reports `false`; the caller then falls
//!   back to `start_kill` on the direct child.
//! * Windows -- process groups cannot be signalled there, so `taskkill /T /F`
//!   walks the parent/child relation the OS already records. A Job Object is the
//!   tighter primitive, but it needs `unsafe` FFI (`win32job`, or `windows-sys`
//!   plus handle plumbing) that this workspace does not depend on, and the
//!   race-free form requires spawning suspended, assigning the job, then
//!   resuming -- a sequence `tokio::process` does not expose. `taskkill` ships
//!   with every supported Windows version and costs one short-lived process per
//!   cancellation.

use std::process::Command;

/// Put the child of `cmd` into a process group of its own.
///
/// Call before `spawn`. Without it there is no group to signal and
/// [`ProcessTree::kill_group`] degrades to killing the direct child only.
pub fn isolate(cmd: &mut Command) {
    imp::isolate(cmd);
}

/// Handle on the process group of one spawned child.
///
/// Create it with [`ProcessTree::adopt`] immediately after `spawn`, while the
/// child id is still reserved and before anything reaps the child. Disarm it
/// once the child is reaped. Dropping an armed handle signals the whole group:
/// that is what turns "the future waiting for the child was dropped" (turn
/// cancel, expired timeout, transport dropped without `close`) into a tree kill
/// instead of an orphan.
///
/// Adoption is only valid for a child spawned from an [`isolate`]d command. For
/// a non-isolated child the pid is not a group id, `killpg` returns `ESRCH`
/// (harmless, it cannot match the parent's group because that group id is
/// already owned by a live leader), and the caller's fallback applies.
#[derive(Debug)]
pub struct ProcessTree {
    /// Child pid, which is also the group id on Unix.
    pid: Option<u32>,
    armed: bool,
}

impl ProcessTree {
    /// Take over the group of a child spawned from an [`isolate`]d command.
    ///
    /// `Child::id` returns `None` once the child is reaped and a reaped pid may
    /// already belong to an unrelated process, so adopt before waiting.
    pub fn adopt(pid: Option<u32>) -> Self {
        Self { pid, armed: true }
    }

    /// The adopted child pid.
    pub fn id(&self) -> Option<u32> {
        self.pid
    }

    /// Signal every process in the group.
    ///
    /// `false` means nothing was delivered: the group is already gone (`ESRCH`),
    /// it was never created, or the platform has no group concept. Callers then
    /// kill the direct child instead, which is why this is not the only path
    /// that must run. Ordering: signal the group, `wait` the direct child, then
    /// [`disarm`](Self::disarm) -- reaping last keeps the pid, and therefore the
    /// group id, reserved while the group signal is in flight.
    pub fn kill_group(&self) -> bool {
        match self.pid {
            Some(pid) => imp::signal_group(pid),
            None => false,
        }
    }

    /// Signal the group, falling back to `direct` when nothing was delivered.
    ///
    /// The fallback is mandatory rather than optional: a child that already
    /// exited with its group leaves `killpg` with `ESRCH`, and skipping
    /// `start_kill` there would turn a successful cancel into a hang on `wait`.
    /// Returns whether the group itself was signalled.
    pub fn kill_tree(&self, direct: impl FnOnce()) -> bool {
        if self.kill_group() {
            return true;
        }
        direct();
        false
    }

    /// Stop signalling on drop.
    ///
    /// Call once the child is reaped: its pid may be reused, and signalling a
    /// recycled group would hit an unrelated process.
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        if self.armed {
            self.kill_group();
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    pub fn isolate(cmd: &mut Command) {
        // Group id 0 means "the child's own pid becomes the group id", so the
        // group stays addressable through the pid already tracked by the guard.
        cmd.process_group(0);
    }

    pub fn signal_group(pid: u32) -> bool {
        // SAFETY: `killpg` reads two integers and sends a signal. An unknown or
        // already-empty group is reported through the return value (`ESRCH`),
        // never through memory unsafety.
        unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) == 0 }
    }
}

#[cfg(windows)]
mod imp {
    use std::process::{Command, Stdio};

    pub fn isolate(_cmd: &mut Command) {
        // `taskkill /T` follows the parent/child relation Windows already
        // records, so spawn needs no extra creation flag.
    }

    pub fn signal_group(pid: u32) -> bool {
        // Returns once the signal has been sent, so this blocking wait stays in
        // the millisecond range even on a cancel path.
        match Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) => status.success(),
            Err(_) => false,
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::process::Command;

    pub fn isolate(_cmd: &mut Command) {}

    pub fn signal_group(_pid: u32) -> bool {
        false
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    /// Fresh scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ark-process-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Spawn `sh -c` whose background job reports its own pid to `pid_file`.
    ///
    /// That background job is the grandchild under test: it survives a
    /// direct-child kill exactly like a leaked browser process does.
    fn spawn_with_grandchild(pid_file: &Path) -> (Child, ProcessTree) {
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        isolate(&mut cmd);
        let child = cmd.spawn().expect("spawn /bin/sh");
        let tree = ProcessTree::adopt(Some(child.id()));
        (child, tree)
    }

    fn is_alive(pid: u32) -> bool {
        // SAFETY: signal 0 performs only the existence/permission check.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    /// Block until the grandchild has written its pid.
    fn read_pid(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "grandchild never reported a pid");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_until_dead(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while is_alive(pid) {
            assert!(
                Instant::now() < deadline,
                "pid {pid} outlived the process group kill"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn kill_group_reaches_the_grandchild() {
        let dir = scratch("kill-group");
        let pid_file = dir.join("grandchild.pid");
        let (mut child, tree) = spawn_with_grandchild(&pid_file);
        let grandchild = read_pid(&pid_file);
        // Kill only the direct child first: that is what a single-process kill
        // did, and the grandchild must survive it. The group signal below is
        // therefore provably what stops it.
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            is_alive(grandchild),
            "grandchild must survive a direct-child kill"
        );
        assert!(tree.kill_group(), "killpg must reach the child's group");
        wait_until_dead(grandchild);
    }

    #[test]
    fn dropping_an_armed_tree_kills_the_grandchild() {
        let dir = scratch("drop");
        let pid_file = dir.join("grandchild.pid");
        let (mut child, tree) = spawn_with_grandchild(&pid_file);
        let grandchild = read_pid(&pid_file);
        // A cancelled call drops the guard along with the child handle; only
        // the guard's drop can still reach the group.
        drop(tree);
        let _ = child.wait();
        wait_until_dead(grandchild);
    }

    #[test]
    fn kill_tree_falls_back_when_the_group_is_gone() {
        // `i32::MAX` exceeds any kernel pid limit, so no group can own it: this
        // is the "group already exited" / `ESRCH` case that must still run the
        // direct kill instead of leaving the child running.
        let tree = ProcessTree::adopt(Some(i32::MAX as u32));
        assert!(!tree.kill_group());
        let mut fell_back = false;
        assert!(!tree.kill_tree(|| fell_back = true));
        assert!(fell_back, "fallback must run when killpg fails");
    }

    #[test]
    fn adopted_pid_is_exposed() {
        assert_eq!(ProcessTree::adopt(Some(42)).id(), Some(42));
        assert_eq!(ProcessTree::adopt(None).id(), None);
    }
}
