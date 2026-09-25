//! A local MCP server as a process: started as its own process group from an
//! empty environment, its stderr kept in a scrubbed ring.
//!
//! None of that output reaches Pluk's log: a server can print a token Pluk
//! never knew was one.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use process_wrap::tokio::{CommandWrap, ProcessGroup};
use rmcp::transport::TokioChildProcess;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{ChildStderr, Command};

use pluk_core::{platform, shell_env};

use super::transport::LaunchSpec;

/// How much output the ring keeps: the last lines, up to this many bytes.
const OUTPUT_LINES: usize = 200;
const OUTPUT_BYTES: usize = 32 * 1024;
/// A line longer than this is kept as several.
const LINE_BYTES: u64 = 4 * 1024;

const REDACTED: &str = "<redacted>";

/// The tail of what a server printed, already scrubbed.
#[derive(Debug, Default)]
pub struct Output {
    lines: VecDeque<String>,
    bytes: usize,
}

impl Output {
    fn push(&mut self, line: String) {
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > OUTPUT_LINES || self.bytes > OUTPUT_BYTES {
            match self.lines.pop_front() {
                Some(dropped) => self.bytes -= dropped.len(),
                None => break,
            }
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

/// Returns the transport, the process group's id, and the server's stderr.
pub fn spawn(spec: &LaunchSpec) -> io::Result<(TokioChildProcess, u32, ChildStderr)> {
    std::fs::create_dir_all(tmp_dir())?;
    let mut command = CommandWrap::from(command(spec));
    command.wrap(ProcessGroup::leader());
    let (transport, stderr) = TokioChildProcess::builder(command)
        .stderr(Stdio::piped())
        .spawn()?;
    let pgid = transport
        .id()
        .ok_or_else(|| io::Error::other("the server exited as it started"))?;
    let stderr = stderr.ok_or_else(|| io::Error::other("the server's output was not piped"))?;
    Ok((transport, pgid, stderr))
}

fn command(spec: &LaunchSpec) -> Command {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(shell_env::base_env(&spec.path))
        .env("TMPDIR", tmp_dir())
        .envs(spec.env.iter().map(|(name, value)| (name, value.expose())));
    command
}

/// macOS clears old files out of the system temp folder, which breaks the
/// packages `bunx` installs there; this one is left alone.
fn tmp_dir() -> PathBuf {
    platform::data_dir().join("mcp-tmp")
}

pub fn drain(stderr: ChildStderr, secrets: Vec<String>, output: Arc<Mutex<Output>>) {
    let mut secrets = secrets;
    // Longest first, so a secret that contains another is scrubbed whole.
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut reader)
                .take(LINE_BYTES)
                .read_until(b'\n', &mut line)
                .await
            {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&line);
                    let scrubbed = scrub(text.trim_end_matches(['\n', '\r']), &secrets);
                    output.lock().expect("server output").push(scrubbed);
                }
            }
        }
    });
}

fn scrub(line: &str, secrets: &[String]) -> String {
    secrets.iter().fold(line.to_string(), |line, secret| {
        line.replace(secret.as_str(), REDACTED)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_last_lines_within_its_budget() {
        let mut output = Output::default();
        for n in 0..(OUTPUT_LINES + 5) {
            output.push(format!("line {n}"));
        }
        let lines = output.lines();
        assert_eq!(lines.len(), OUTPUT_LINES);
        assert_eq!(lines.last().map(String::as_str), Some("line 204"));

        let mut wide = Output::default();
        for _ in 0..20 {
            wide.push("x".repeat(4 * 1024));
        }
        assert!(wide.bytes <= OUTPUT_BYTES);
        assert_eq!(wide.lines().len(), OUTPUT_BYTES / (4 * 1024));
    }

    #[test]
    fn every_secret_is_scrubbed_and_a_longer_one_whole() {
        let mut secrets = vec!["abc".to_string(), "abc-123".to_string()];
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        assert_eq!(
            scrub("token=abc-123 short=abc", &secrets),
            "token=<redacted> short=<redacted>"
        );
    }
}
