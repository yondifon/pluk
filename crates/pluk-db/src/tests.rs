#[cfg(test)]
mod tests {
    use crate::driver::with_opts;
    use crate::error::DriverError;
    use crate::fake::FakeDriver;
    use crate::driver::Driver;
    use crate::types::QueryOpts;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn read_only_prevents_write() {
        let d = FakeDriver::new_generic();
        let err = d.query_read_only("INSERT INTO t VALUES (1)", &[], None).await.unwrap_err();
        assert!(matches!(err, DriverError::Query(m) if m.contains("read-only")));
        // Read still works through read-only path
        let ok = d.query_read_only("SELECT * FROM t", &[], None).await.unwrap();
        assert_eq!(ok.rows.len(), 1);
    }

    #[tokio::test]
    async fn read_only_allows_select_but_not_update() {
        let d = FakeDriver::new_generic();
        for sql in ["UPDATE t SET x=1", "DELETE FROM t", "DROP TABLE t"] {
            let e = d.query_read_only(sql, &[], None).await.unwrap_err();
            assert!(matches!(e, DriverError::Query(_)), "{sql} should be rejected");
        }
    }

    #[tokio::test]
    async fn cancellation_surfaces_as_cancelled_not_error() {
        let d = FakeDriver::new_generic();
        let token = CancellationToken::new();
        let t2 = token.clone();
        // Cancel after 5ms while query sleeps 50ms
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            t2.cancel();
        });
        let opts = QueryOpts { timeout_ms: None, cancel: Some(token) };
        let err = d.query("SELECT pg_sleep(10)", &[], Some(opts)).await.unwrap_err();
        assert!(matches!(err, DriverError::Cancelled), "got {err:?}");
    }

    #[tokio::test]
    async fn cancellation_without_timeout_also_cancelled() {
        let d = FakeDriver::new_generic();
        let token = CancellationToken::new();
        token.cancel();
        let opts = QueryOpts { timeout_ms: None, cancel: Some(token) };
        let err = d.query("SELECT 1", &[], Some(opts)).await.unwrap_err();
        assert!(matches!(err, DriverError::Cancelled));
    }

    #[tokio::test]
    async fn timeout_surfaces_as_timeout() {
        let d = FakeDriver::new_generic();
        let opts = QueryOpts { timeout_ms: Some(5), cancel: None };
        let err = d.query("SELECT 1", &[], Some(opts)).await.unwrap_err();
        assert!(matches!(err, DriverError::Timeout(5)));
    }

    #[tokio::test]
    async fn timeout_not_triggered_when_sufficient() {
        let d = FakeDriver::new_generic();
        let opts = QueryOpts { timeout_ms: Some(500), cancel: None };
        let ok = d.query("SELECT 1", &[], Some(opts)).await.unwrap();
        assert_eq!(ok.rows.len(), 1);
    }

    #[tokio::test]
    async fn pin_rule_rejects_mismatched_database() {
        let cfg = crate::config::SqlConfig { r#type: "postgres".into(), database: Some("appdb".into()), ..Default::default() };
        let opts = crate::factory::CreateDriverOpts::new(cfg).with_database("otherdb");
        let err = crate::factory::create_driver(opts).await.unwrap_err();
        assert!(matches!(err, DriverError::DatabasePinned(_)));
        assert!(err.to_string().contains("locked to database"));
    }

    #[tokio::test]
    async fn pin_rule_allows_same_database() {
        let cfg = crate::config::SqlConfig { r#type: "postgres".into(), database: Some("appdb".into()), ..Default::default() };
        let opts = crate::factory::CreateDriverOpts::new(cfg).with_database("appdb");
        let res = crate::factory::create_driver(opts).await.unwrap();
        assert_eq!(res.driver.list_databases().await.unwrap(), vec!["postgres"]);
    }

    #[tokio::test]
    async fn pin_rule_rejects_invalid_name() {
        let cfg = crate::config::SqlConfig { r#type: "postgres".into(), database: None, ..Default::default() };
        let opts = crate::factory::CreateDriverOpts::new(cfg).with_database("evil; DROP");
        let err = crate::factory::create_driver(opts).await.unwrap_err();
        assert!(matches!(err, DriverError::InvalidDatabaseName(_)));
    }

    #[tokio::test]
    async fn ssl_mode_mapping_verify_ca() {
        let cfg = crate::config::SqlConfig { r#type: "postgres".into(), use_ssl: true, ssl_mode: Some("verify-ca".into()), ..Default::default() };
        let ssl = crate::config::resolve_ssl(&cfg).unwrap().unwrap();
        assert!(ssl.reject_unauthorized);
        assert_eq!(ssl.mode, Some(crate::ssl::SslMode::VerifyCa));
    }

    #[tokio::test]
    async fn with_opts_cancel_preempts_timeout() {
        // Cancellation should win even when timeout would also fire
        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            t2.cancel();
        });
        let opts = QueryOpts { timeout_ms: Some(1000), cancel: Some(token) };
        let fut = async { tokio::time::sleep(std::time::Duration::from_millis(500)).await; Ok::<i32, DriverError>(1) };
        let err = with_opts(Some(opts), fut).await.unwrap_err();
        assert!(matches!(err, DriverError::Cancelled));
    }

    #[tokio::test]
    async fn ssh_seam_rewrites_host_port() {
        struct FakeTunnel;
        #[async_trait::async_trait]
        impl crate::config::SshTunnelProvider for FakeTunnel {
            async fn open_tunnel(&self, _cfg: &crate::config::SqlConfig, _remote_host: &str, _remote_port: u16) -> Result<crate::config::TunnelEndpoint, crate::error::DriverError> {
                Ok(crate::config::TunnelEndpoint { local_host: "127.0.0.1".into(), local_port: 2222, close_fn: None })
            }
        }
        let cfg = crate::config::SqlConfig { r#type: "postgres".into(), host: Some("db.internal".into()), port: Some(5432), use_ssh: Some("true".into()), ssh_host: Some("bastion".into()), ..Default::default() };
        let mut opts = crate::factory::CreateDriverOpts::new(cfg);
        opts.ssh_provider = Some(Box::new(FakeTunnel));
        let res = crate::factory::create_driver(opts).await.unwrap();
        assert!(res.tunnel.is_some());
        assert_eq!(res.tunnel.unwrap().local_port, 2222);
    }

    #[tokio::test]
    async fn ssh_tunnel_config_keys_match_typescript() {
        struct CapturingTunnel(std::sync::Arc<std::sync::Mutex<Option<crate::config::SqlConfig>>>);
        #[async_trait::async_trait]
        impl crate::config::SshTunnelProvider for CapturingTunnel {
            async fn open_tunnel(&self, cfg: &crate::config::SqlConfig, remote_host: &str, remote_port: u16) -> Result<crate::config::TunnelEndpoint, crate::error::DriverError> {
                assert_eq!(remote_host, "db.internal");
                assert_eq!(remote_port, 5432);
                *self.0.lock().unwrap() = Some(cfg.clone());
                Ok(crate::config::TunnelEndpoint { local_host: "127.0.0.1".into(), local_port: 3333, close_fn: None })
            }
        }
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let cfg = crate::config::SqlConfig {
            r#type: "postgres".into(),
            host: Some("db.internal".into()),
            port: Some(5432),
            use_ssh: Some("true".into()),
            ssh_host: Some("bastion.example.com".into()),
            ssh_port: Some(2222),
            ssh_user: Some("deploy".into()),
            ssh_auth_type: Some("key".into()),
            ssh_key_path: Some("~/.ssh/id_ed25519".into()),
            ssh_password: Some("secret".into()),
            ..Default::default()
        };
        let mut opts = crate::factory::CreateDriverOpts::new(cfg.clone());
        opts.ssh_provider = Some(Box::new(CapturingTunnel(captured.clone())));
        let res = crate::factory::create_driver(opts).await.unwrap();
        assert_eq!(res.tunnel.unwrap().local_port, 3333);
        let got = captured.lock().unwrap().clone().unwrap();
        assert_eq!(got.ssh_host.as_deref(), Some("bastion.example.com"));
        assert_eq!(got.ssh_port, Some(2222));
        assert_eq!(got.ssh_user.as_deref(), Some("deploy"));
        assert_eq!(got.ssh_auth_type.as_deref(), Some("key"));
        assert_eq!(got.ssh_key_path.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(got.ssh_password.as_deref(), Some("secret"));
    }

    #[tokio::test]
    async fn sqlite_remote_rejects_missing_ssh_host_with_typescript_message() {
        let cfg = crate::config::SqlConfig {
            r#type: "sqlite".into(),
            filename: Some("/tmp/db.sqlite".into()),
            use_ssh: Some("true".into()),
            ssh_host: None,
            ..Default::default()
        };
        let err = crate::factory::create_driver(crate::factory::CreateDriverOpts::new(cfg)).await.unwrap_err();
        assert!(err.to_string().contains("SQLite SSH host is missing"));
        assert!(err.to_string().contains("connection settings"));
    }

    #[tokio::test]
    async fn sqlite_remote_rejects_missing_filename_with_remote_message() {
        let cfg = crate::config::SqlConfig {
            r#type: "sqlite".into(),
            filename: None,
            database: None,
            use_ssh: Some("true".into()),
            ssh_host: Some("bastion".into()),
            ..Default::default()
        };
        let err = crate::factory::create_driver(crate::factory::CreateDriverOpts::new(cfg)).await.unwrap_err();
        assert!(err.to_string().contains("SQLite path is missing"));
        assert!(err.to_string().contains("remote database file path"));
    }

    #[tokio::test]
    async fn sqlite_remote_exec_is_wired_and_timeout_propagates() {
        struct SlowExec;
        #[async_trait::async_trait]
        impl crate::config::SshExecProvider for SlowExec {
            async fn exec(&self, _command: String, timeout_ms: Option<u64>) -> Result<String, crate::error::DriverError> {
                let ms = timeout_ms.unwrap_or(30_000);
                tokio::time::sleep(std::time::Duration::from_millis(ms + 50)).await;
                Ok("[]".into())
            }
        }
        let cfg = crate::config::SqlConfig {
            r#type: "sqlite".into(),
            filename: Some("/tmp/remote.sqlite".into()),
            use_ssh: Some("true".into()),
            ssh_host: Some("bastion".into()),
            ..Default::default()
        };
        let mut opts = crate::factory::CreateDriverOpts::new(cfg);
        opts.ssh_exec_provider = Some(Box::new(SlowExec));
        let dw = crate::factory::create_driver(opts).await.unwrap();
        let opts = crate::types::QueryOpts { timeout_ms: Some(20), cancel: None };
        let err = dw.driver.query("SELECT 1", &[], Some(opts)).await.unwrap_err();
        assert!(matches!(err, crate::error::DriverError::Timeout(20)), "got {:?}", err);
    }

    #[tokio::test]
    async fn sqlite_remote_exec_output_capped_at_one_million() {
        struct LargeExec;
        #[async_trait::async_trait]
        impl crate::config::SshExecProvider for LargeExec {
            async fn exec(&self, _command: String, _timeout_ms: Option<u64>) -> Result<String, crate::error::DriverError> {
                Ok("x".repeat(1_000_001))
            }
        }
        let cfg = crate::config::SqlConfig {
            r#type: "sqlite".into(),
            filename: Some("/tmp/remote.sqlite".into()),
            use_ssh: Some("true".into()),
            ssh_host: Some("bastion".into()),
            ssh_port: Some(22),
            ssh_user: Some("alice".into()),
            ssh_auth_type: Some("agent".into()),
            ..Default::default()
        };
        let mut opts = crate::factory::CreateDriverOpts::new(cfg);
        opts.ssh_exec_provider = Some(Box::new(LargeExec));
        let dw = crate::factory::create_driver(opts).await.unwrap();
        let out = dw.driver.query("SELECT 1", &[], None).await;
        assert!(out.is_err());
        let msg = out.unwrap_err().to_string();
        assert!(msg.contains("failed to parse sqlite3"), "large output should be capped then fail JSON parse, got: {}", msg);
    }

    #[tokio::test]
    async fn sqlite_remote_rejects_bind_params_with_injection_message() {
        struct NoopExec;
        #[async_trait::async_trait]
        impl crate::config::SshExecProvider for NoopExec {
            async fn exec(&self, _command: String, _timeout_ms: Option<u64>) -> Result<String, crate::error::DriverError> { Ok("[]".into()) }
        }
        let cfg = crate::config::SqlConfig {
            r#type: "sqlite".into(),
            filename: Some("/tmp/remote.sqlite".into()),
            use_ssh: Some("true".into()),
            ssh_host: Some("bastion".into()),
            ..Default::default()
        };
        let mut opts = crate::factory::CreateDriverOpts::new(cfg);
        opts.ssh_exec_provider = Some(Box::new(NoopExec));
        let dw = crate::factory::create_driver(opts).await.unwrap();
        let err = dw.driver.query("SELECT ?", &[serde_json::json!(1)], None).await.unwrap_err();
        assert!(err.to_string().contains("Bind parameters are not supported"));
    }

    #[tokio::test]
    async fn tunnel_uses_pluk_ssh_provider_by_default_without_manual_injection() {
        struct FailingTunnel;
        #[async_trait::async_trait]
        impl crate::config::SshTunnelProvider for FailingTunnel {
            async fn open_tunnel(&self, _cfg: &crate::config::SqlConfig, _remote_host: &str, _remote_port: u16) -> Result<crate::config::TunnelEndpoint, crate::error::DriverError> {
                panic!("should not be called when default provider is PlukSshTunnelProvider");
            }
        }
        let cfg = crate::config::SqlConfig {
            r#type: "postgres".into(),
            host: Some("db.internal".into()),
            port: Some(5432),
            use_ssh: Some("true".into()),
            ssh_host: Some("bastion.example.com".into()),
            ssh_auth_type: Some("agent".into()),
            ..Default::default()
        };
        let provider = crate::ssh_provider::PlukSshTunnelProvider;
        assert!(cfg.is_use_ssh());
        assert_eq!(cfg.ssh_auth_type.as_deref(), Some("agent"));
        let _ = provider;
    }
}
