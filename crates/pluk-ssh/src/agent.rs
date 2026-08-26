
pub const SSH_AGENT_UNREACHABLE_CODE: &str = "SSH_AGENT_UNREACHABLE";

const PROBE_TIMEOUT_MS: u64 = 2_000;
const SSH_AGENTC_REQUEST_IDENTITIES: u8 = 11;
const SSH_AGENT_IDENTITIES_ANSWER: u8 = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProbe {
    Keys { keys: u32 },
    Empty,
    Mute,
    Dead { error: String },
}

impl AgentProbe {
    pub fn state_str(&self) -> &'static str {
        match self {
            Self::Keys { .. } => "keys",
            Self::Empty => "empty",
            Self::Mute => "mute",
            Self::Dead { .. } => "dead",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveAgent {
    pub socket: String,
    pub probe: AgentProbe,
}

/// Probe an agent socket by actually connecting and sending
/// `SSH_AGENTC_REQUEST_IDENTITIES`. The probe itself wakes a locked 1Password,
/// so it must connect rather than just stat the path.
pub async fn probe_agent_socket(path: &str, timeout_ms: u64) -> AgentProbe {
    let timeout = std::time::Duration::from_millis(timeout_ms);
    tokio::time::timeout(timeout, probe_inner(path))
        .await
        .unwrap_or(AgentProbe::Dead {
            error: "connect timed out".into(),
        })
}

async fn probe_inner(path: &str) -> AgentProbe {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = match tokio::net::UnixStream::connect(path).await {
        Ok(s) => s,
        Err(e) => {
            return AgentProbe::Dead {
                error: e.to_string(),
            }
        }
    };

    // Send SSH_AGENTC_REQUEST_IDENTITIES: 4-byte len + 1 byte type
    let req = [0u8, 0, 0, 1, SSH_AGENTC_REQUEST_IDENTITIES];
    if let Err(e) = stream.write_all(&req).await {
        return AgentProbe::Dead {
            error: e.to_string(),
        };
    }

    // Wait for response with a short read timeout
    let mut buf = vec![0u8; 8192];
    let read_fut = stream.read(&mut buf);
    let n = match tokio::time::timeout(std::time::Duration::from_millis(PROBE_TIMEOUT_MS), read_fut).await {
        Ok(Ok(0)) => {
            return AgentProbe::Dead {
                error: "agent closed the connection".into(),
            }
        }
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            return AgentProbe::Dead {
                error: e.to_string(),
            }
        }
        Err(_) => return AgentProbe::Mute,
    };

    if n < 5 {
        return AgentProbe::Mute;
    }
    if buf[4] != SSH_AGENT_IDENTITIES_ANSWER {
        return AgentProbe::Mute;
    }
    if n < 9 {
        return AgentProbe::Mute;
    }
    let keys = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]);
    if keys > 0 {
        AgentProbe::Keys { keys }
    } else {
        AgentProbe::Empty
    }
}

fn well_known_agent_sockets() -> Vec<String> {
    let home = match pluk_core::platform::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let candidates = [
        home.join("Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"),
        home.join(".1password/agent.sock"),
    ];
    candidates
        .into_iter()
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
        .collect()
}

pub fn agent_socket_candidates(host: &str) -> Vec<String> {
    let from_config = crate::config::parse_ssh_config(host)
        .identity_agent
        .filter(|s| !s.is_empty());
    let mut all: Vec<String> = Vec::new();
    if let Some(s) = from_config {
        all.push(s);
    }
    all.extend(well_known_agent_sockets());
    if let Ok(sock) = std::env::var("SSH_AUTH_SOCK")
        && !sock.is_empty() {
            all.push(sock);
        }
    // Deduplicate preserving order
    let mut seen = std::collections::HashSet::new();
    all.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

/// Prefer an agent that answered with keys; otherwise a mute one (locked
/// 1Password — the probe just asked it to unlock). Empty/dead are not picked.
pub fn pick_live_agent(probed: &[LiveAgent]) -> Option<LiveAgent> {
    // First try to find a mute one (locked 1Password — worth waiting on)
    probed.iter().find(|p| p.probe == AgentProbe::Mute).cloned()
}

pub async fn resolve_live_agent(host: &str) -> Option<LiveAgent> {
    let mut probed: Vec<LiveAgent> = Vec::new();
    for socket in agent_socket_candidates(host) {
        let probe = probe_agent_socket(&socket, PROBE_TIMEOUT_MS).await;
        eprintln!(
            "[pluk] SSH agent probe: {} -> {}{}",
            socket,
            probe.state_str(),
            match &probe {
                AgentProbe::Dead { error } => format!(" ({error})"),
                _ => String::new(),
            }
        );
        match &probe {
            AgentProbe::Keys { .. } => return Some(LiveAgent { socket, probe }),
            AgentProbe::Dead { .. } => {}
            _ => probed.push(LiveAgent {
                socket: socket.clone(),
                probe,
            }),
        }
    }
    pick_live_agent(&probed)
}

#[derive(Debug, thiserror::Error)]
#[error("{}", message)]
pub struct AgentUnreachableError {
    pub message: String,
    pub code: &'static str,
}

pub fn agent_unreachable_error() -> AgentUnreachableError {
    AgentUnreachableError {
        message: "Can't reach your SSH key agent — no agent socket answered, so no approval prompt can appear. Open and unlock 1Password (with its SSH agent enabled), or load the key into ssh-agent, then retry.".into(),
        code: SSH_AGENT_UNREACHABLE_CODE,
    }
}

impl AgentUnreachableError {
    pub fn to_std_error(&self) -> std::io::Error {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, self.message.clone());
        // Attach code via display; callers check via is_ssh_auth_error
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_prefers_mute_over_empty() {
        let agents = vec![
            LiveAgent {
                socket: "/tmp/a.sock".into(),
                probe: AgentProbe::Empty,
            },
            LiveAgent {
                socket: "/tmp/b.sock".into(),
                probe: AgentProbe::Mute,
            },
        ];
        assert_eq!(pick_live_agent(&agents).unwrap().socket, "/tmp/b.sock");
    }

    #[test]
    fn pick_returns_none_when_all_dead_or_empty() {
        let agents = vec![LiveAgent {
            socket: "/tmp/a.sock".into(),
            probe: AgentProbe::Empty,
        }];
        assert!(pick_live_agent(&agents).is_none());
        assert!(pick_live_agent(&[]).is_none());
    }

    #[test]
    fn agent_unreachable_has_code() {
        let e = agent_unreachable_error();
        assert_eq!(e.code, SSH_AGENT_UNREACHABLE_CODE);
        assert!(e.message.contains("1Password"));
    }

    #[tokio::test]
    async fn probe_nonexistent_is_dead() {
        let probe = probe_agent_socket("/tmp/pluk-test-nonexistent-xyz.sock", 200).await;
        assert!(matches!(probe, AgentProbe::Dead { .. }));
    }
}
