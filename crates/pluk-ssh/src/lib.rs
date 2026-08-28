pub mod agent;
pub mod config;
pub mod openssh;
pub mod pending;
pub mod pool;
pub mod russh_client;
pub mod tunnel;

pub use agent::{
    SSH_AGENT_UNREACHABLE_CODE, agent_unreachable_error, probe_agent_socket, resolve_live_agent,
};
pub use config::{
    expand_home, expand_proxy_command, parse_ssh_config, resolve_agent_socket, split_command,
};
pub use openssh::{
    HANDSHAKE_TIMEOUT_MS, SshError, SshTunnelConfig, Tunnel, control_dir, control_path,
};
pub use pending::{
    SSH_CONNECT_WAIT_MS, SSH_PENDING_CODE, SSH_PENDING_MAX_REPORTS, SSH_STALLED_CODE,
    clear_connect_episode, connect_wait_error, is_ssh_auth_error, is_ssh_pending, is_ssh_stalled,
    is_transient_ssh_error, record_connect_failure_msg, ssh_pending_error, ssh_stalled_error,
    start_connect_attempt,
};
pub use tunnel::open_ssh_tunnel;
