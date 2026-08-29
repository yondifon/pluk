//! Running child processes for the CLI-backed adapters.
//!
//! Every child leads its own process group, so a timeout or a dropped call
//! terminates the command *and* whatever it started. Without that, Pluk reports
//! a timeout while the remote mutation the child was performing keeps going.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

/// What a finished child produced.
#[derive(Debug, Clone)]
pub struct Captured {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub enum RunError {
    /// The command could not be started.
    Spawn(std::io::Error),
    /// The child started but its output or exit status could not be read.
    Io(std::io::Error),
    /// The child outlived the timeout; its process group has been killed.
    TimedOut,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) | Self::Io(e) => write!(f, "{e}"),
            Self::TimedOut => write!(f, "timed out"),
        }
    }
}

impl std::error::Error for RunError {}

/// Run `cmd` to completion, capturing stdout and stderr.
///
/// Stdio is set here: stdin is closed so no child can block on a prompt, and
/// both output streams are captured. If the timeout elapses or the returned
/// future is dropped, the child's whole process group is killed before this
/// call gives up on it.
pub async fn run_capture(cmd: &mut Command, timeout: Duration) -> Result<Captured, RunError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let child = cmd.spawn().map_err(RunError::Spawn)?;
    let mut group = ProcessGroup::new(child);
    match tokio::time::timeout(timeout, group.wait()).await {
        Ok(res) => res.map_err(RunError::Io),
        Err(_) => Err(RunError::TimedOut),
    }
}

/// A spawned child plus the promise that its group dies with this value.
struct ProcessGroup {
    child: Child,
    /// The group leader's pid while the child is still ours to kill; cleared
    /// once it has been waited on and the pid can be reused.
    leader: Option<u32>,
}

impl ProcessGroup {
    fn new(child: Child) -> Self {
        let leader = child.id();
        Self { child, leader }
    }

    async fn wait(&mut self) -> std::io::Result<Captured> {
        let mut stdout = self.child.stdout.take();
        let mut stderr = self.child.stderr.take();
        let (status, out, err) = tokio::join!(
            self.child.wait(),
            read_pipe(&mut stdout),
            read_pipe(&mut stderr),
        );
        let status = status?;
        self.leader = None;
        Ok(Captured {
            code: status.code(),
            stdout: out?,
            stderr: err?,
        })
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.leader {
            crate::platform::kill_process_group(pid);
        }
    }
}

async fn read_pipe<R: AsyncRead + Unpin>(pipe: &mut Option<R>) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    if let Some(pipe) = pipe {
        pipe.read_to_end(&mut buf).await?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell that starts a background sleep, records its pid, and then sleeps
    /// itself — so the tree outlives the process Pluk spawned.
    fn tree_command(pid_file: &std::path::Path) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(format!(
            "sleep 30 & echo $! > {}; sleep 30",
            pid_file.display()
        ));
        cmd
    }

    async fn recorded_pid(pid_file: &std::path::Path) -> i32 {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(pid_file)
                && let Ok(pid) = text.trim().parse::<i32>()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("child never recorded its grandchild pid");
    }

    /// `kill -0` succeeds only while the process is alive and unreaped.
    async fn alive(pid: i32) -> bool {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    async fn wait_until_gone(pid: i32) -> bool {
        for _ in 0..100 {
            if !alive(pid).await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test]
    async fn captures_output_and_exit_code() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("echo out; echo err >&2; exit 3");
        let out = run_capture(&mut cmd, Duration::from_secs(10)).await.unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "out");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "err");
    }

    #[tokio::test]
    async fn timeout_kills_the_whole_tree() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let mut cmd = tree_command(&pid_file);

        let err = run_capture(&mut cmd, Duration::from_millis(400))
            .await
            .unwrap_err();
        assert!(matches!(err, RunError::TimedOut), "got {err:?}");

        let grandchild = recorded_pid(&pid_file).await;
        assert!(
            wait_until_gone(grandchild).await,
            "grandchild {grandchild} survived the timeout"
        );
    }

    #[tokio::test]
    async fn dropping_the_call_kills_the_whole_tree() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let mut cmd = tree_command(&pid_file);

        // Let it spawn, then abandon the call the way a cancelled request does:
        // the future is dropped at the end of this block.
        let grandchild = {
            let run = run_capture(&mut cmd, Duration::from_secs(30));
            tokio::pin!(run);
            assert!(
                tokio::time::timeout(Duration::from_millis(400), &mut run)
                    .await
                    .is_err()
            );
            recorded_pid(&pid_file).await
        };

        assert!(
            wait_until_gone(grandchild).await,
            "grandchild {grandchild} survived cancellation"
        );
    }
}
