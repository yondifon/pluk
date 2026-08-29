//! Helpers for the adapter tests that check a timed-out CLI leaves nothing running.

use std::path::Path;
use std::time::Duration;

/// A shell that starts a background sleep, records its pid, then sleeps itself —
/// so the tree outlives the process the adapter spawned.
pub fn tree_script(pid_file: &Path) -> String {
    format!("sleep 30 & echo $! > {}; sleep 30", pid_file.display())
}

pub async fn recorded_pid(pid_file: &Path) -> i32 {
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
pub async fn wait_until_gone(pid: i32) -> bool {
    for _ in 0..100 {
        let alive = tokio::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if !alive {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}
