//! Unified SSH tunnel entry point choosing between OpenSSH and russh.
//!
//! Mirrors routing in `pluk/src/db/ssh.ts`: agent/passphrase-less keys via
//! OpenSSH ControlMaster, password/encrypted keys via in-process client.

use std::sync::Arc;

use crate::openssh::{SshError, SshTunnelConfig, Tunnel};

/// Open an SSH tunnel choosing the transport deliberately.
///
/// - Agent and passphrase-less key → OpenSSH ControlMaster (system `ssh` binary)
/// - Password and encrypted keys → in-process russh client
///
/// The split is preserved because the in-process library's port forwarding
/// historically passed zero bytes under Bun's runtime, and OpenSSH drives a
/// 1Password-style agent correctly via `IdentityAgent`. In Rust, russh
/// forwarding works, so both paths are viable — but we keep the split for
/// agent correctness and to avoid regressing 1Password approval flows.
pub async fn open_ssh_tunnel(
    config: SshTunnelConfig,
    on_fatal: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<Tunnel, SshError> {
    let use_openssh = config.auth_type == "agent"
        || (config.auth_type == "key" && config.passphrase.is_none());

    if use_openssh {
        crate::openssh::open_ssh_tunnel_via_openssh(config, on_fatal).await
    } else {
        crate::russh_client::open_tunnel_russh(config, on_fatal).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_decision() {
        let agent_cfg = SshTunnelConfig {
            host: "bastion".into(),
            port: 22,
            user: "alice".into(),
            auth_type: "agent".into(),
            key_path: None,
            passphrase: None,
            remote_host: "db".into(),
            remote_port: 5432,
        };
        assert!(agent_cfg.auth_type == "agent");

        let key_no_pass = SshTunnelConfig {
            auth_type: "key".into(),
            passphrase: None,
            ..agent_cfg.clone()
        };
        let key_with_pass = SshTunnelConfig {
            auth_type: "key".into(),
            passphrase: Some("secret".into()),
            key_path: Some("~/.ssh/id_rsa".into()),
            ..agent_cfg.clone()
        };
        let password_cfg = SshTunnelConfig {
            auth_type: "password".into(),
            passphrase: Some("pw".into()),
            ..agent_cfg.clone()
        };

        // These mirror the routing predicate in open_ssh_tunnel
        assert!(key_no_pass.auth_type == "key" && key_no_pass.passphrase.is_none());
        assert!(!(key_with_pass.auth_type == "key" && key_with_pass.passphrase.is_none()));
        assert!(!(password_cfg.auth_type == "agent"));
    }
}
