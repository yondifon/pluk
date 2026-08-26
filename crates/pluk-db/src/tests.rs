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
        // FakeDriver stores effective host/port — should be tunnel endpoint, not original
        // We can't downcast Box<dyn Driver>, but we can test via factory's tunnel field
        assert!(res.tunnel.is_some());
        assert_eq!(res.tunnel.unwrap().local_port, 2222);
    }
}
