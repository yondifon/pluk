use super::client::{
    ExecResult, Forward, ForwardOpener, StubExecutor, clear_test_executor, close_forward,
    list_forwards, open_forward, reset_forwards_for_test, set_test_executor,
    set_test_forward_opener,
};
use super::error::humanize_ssh_error;
use super::policy::{CommandCategory, evaluate_command, policy_summary};
use super::server::{register_ssh_server, ssh_instructions, ssh_tool_specs};
use crate::error::AdapterError;
use crate::tool_host::{ToolHost, ToolRegistration};
use pluk_ssh::SshTunnelConfig;
use pluk_store::{Integration, Store};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The executor and transport seams are process-global, so the tests that
/// install one run in sequence.
static SSH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn temp_store() -> (tempfile::TempDir, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&dir.path().join("pluk.db")).unwrap());
    (dir, store)
}

fn make_integration(id: &str, config: Value) -> Integration {
    make_integration_with_policy(id, config, None)
}
fn make_integration_with_policy(id: &str, config: Value, policy: Option<&str>) -> Integration {
    let mut cfg = Map::new();
    if let Value::Object(m) = config {
        cfg = m;
    }
    Integration {
        id: id.to_string(),
        name: "Test SSH".to_string(),
        r#type: "ssh".to_string(),
        config: cfg,
        query_policy: policy.map(|s| s.to_string()),
        environment: None,
        via_group: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        read_only: 0,
        token: "test-token".to_string(),
    }
}
fn ssh_all_enabled_policy() -> &'static str {
    r#"{"tools":{"run_batch":{"enabled":true},"debug_snapshot":{"enabled":true},"run_saved_command":{"enabled":true},"list_saved_commands":{"enabled":true},"open_forward":{"enabled":true},"list_forwards":{"enabled":true},"close_forward":{"enabled":true}}}"#
}

// Simple host capture
struct CaptureHost {
    tools: HashMap<String, (ToolRegistration, crate::tool_host::ToolHandler)>,
}
impl ToolHost for CaptureHost {
    fn register_tool(&mut self, reg: ToolRegistration, handler: crate::tool_host::ToolHandler) {
        self.tools.insert(reg.name.clone(), (reg, handler));
    }
    fn register_prompt(
        &mut self,
        _name: &str,
        _desc: &str,
        _schema: Option<Map<String, Value>>,
        _handler: crate::tool_host::PromptHandler,
    ) {
    }
    fn register_resource(
        &mut self,
        _name: &str,
        _uri: &str,
        _mime: &str,
        _desc: Option<&str>,
        _handler: crate::tool_host::ResourceHandler,
    ) {
    }
}

#[test]
fn policy_each_blocked_path_pattern() {
    let cases = vec![
        ("cat .env", true),
        ("cat .env.local", true),
        ("cat .env.production", true),
        ("cat .env/foobar", true),
        ("cat id_rsa", true),
        ("cat id_ed25519", true),
        ("cat secrets.pem", true),
        ("cat foo.key", true),
        ("cat /home/user/.ssh/id_rsa", true),
        ("cat ~/.aws/credentials", true),
        ("cat ~/.gnupg/pubring", true),
        ("cat ~/.netrc", true),
        ("cat ~/.npmrc", true),
        ("cat /etc/shadow", true),
        ("cat /etc/gshadow", true),
        ("cat /etc/sudoers", true),
        ("cat credentials", true),
    ];
    for (cmd, should_block) in cases {
        let v = evaluate_command(cmd);
        assert_eq!(
            !v.ok, should_block,
            "cmd {} should block={} but got {:?}",
            cmd, should_block, v
        );
    }
}

#[test]
fn policy_brace_expansion_smuggling() {
    assert!(!evaluate_command("cat {.env,foo}").ok);
    assert!(!evaluate_command("cat {a,b}.pem").ok);
    assert!(evaluate_command("docker ps --format {{.Names}}").ok);
    assert!(evaluate_command("echo {{test}}").ok);
}

#[test]
fn policy_deliberately_excluded_commands() {
    for cmd in [
        "env",
        "printenv",
        "curl https://example.com",
        "wget http://example.com",
        "bash -c ls",
        "sh -c 'ls'",
        "python -c 'print(1)'",
        "perl -e '1'",
    ] {
        assert!(!evaluate_command(cmd).ok, "{} should be blocked", cmd);
    }
}

#[test]
fn policy_metacharacters_blocked_and_pipes_allowed() {
    assert!(!evaluate_command("ls; rm").ok);
    assert!(!evaluate_command("ls && echo hi").ok);
    assert!(!evaluate_command("ls || echo hi").ok);
    assert!(!evaluate_command("echo `whoami`").ok);
    assert!(!evaluate_command("echo $(whoami)").ok);
    assert!(!evaluate_command("ls > /tmp/out").ok);
    assert!(evaluate_command("ps aux | grep nginx").ok);
    assert!(evaluate_command("cat /var/log/syslog | grep error | wc -l").ok);
}

#[test]
fn policy_write_detection() {
    assert_eq!(
        evaluate_command("docker-compose up").category,
        CommandCategory::Write
    );
    assert_eq!(
        evaluate_command("docker-compose ps").category,
        CommandCategory::Read
    );
    assert_eq!(evaluate_command("ls").category, CommandCategory::Read);
}

#[test]
fn tool_specs_defaults() {
    let specs = ssh_tool_specs();
    let map: HashMap<_, _> = specs
        .into_iter()
        .map(|s| (s.name, s.default_enabled))
        .collect();
    assert!(map["run_command"]);
    for k in [
        "run_batch",
        "debug_snapshot",
        "run_saved_command",
        "list_saved_commands",
        "open_forward",
        "list_forwards",
        "close_forward",
    ] {
        assert!(!map[k], "{} should be default off", k);
    }
}

#[tokio::test]
async fn timeout_enforcement_and_humanize() {
    let _guard = SSH_TEST_LOCK.lock().await;
    let (_dir, store) = temp_store();
    let conn = make_integration(
        "ssh1",
        json!({"host":"example.com","port":22,"user":"alice","auth_type":"agent"}),
    );
    // stub executor that simulates timeout error
    let exec = Arc::new(StubExecutor {
        handler: Arc::new(|cmd, timeout_ms| {
            if cmd.contains("sleep") && timeout_ms < 2000 {
                Err(AdapterError::new(format!(
                    "Command timed out after {}s",
                    timeout_ms / 1000
                )))
            } else {
                Ok(ExecResult {
                    stdout: "ok".into(),
                    stderr: "".into(),
                    code: Some(0),
                    truncated: false,
                })
            }
        }),
    });
    set_test_executor(exec);
    // Direct client call with timeout 1s
    let res = super::client::run_command(&conn, "sleep 10", Some(1000)).await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    let human = humanize_ssh_error(&err);
    assert!(
        human.contains("retry with a higher"),
        "humanize should hint retry: {}",
        human
    );
    // max timeout check via tool validation: register and call with 700
    let mut host = CaptureHost {
        tools: HashMap::new(),
    };
    register_ssh_server(&mut host, &conn, "owner1", store.clone()).unwrap();
    let handler = host.tools.get("run_command").unwrap().1.clone();
    let args = json!({"command":"ls","timeout": 700});
    let result = handler(args).await;
    assert!(result.is_error);
    assert!(result.text().contains("timeout must be <="));
    clear_test_executor();
    reset_forwards_for_test();
}

/// Stands in for the SSH transport: hands back the requested local port, or the
/// next auto-assigned one, and records what it was asked to open.
struct StubOpener {
    next_port: Mutex<u16>,
    requests: Arc<Mutex<Vec<SshTunnelConfig>>>,
    closes: Arc<AtomicUsize>,
}

impl StubOpener {
    fn new(first_port: u16) -> Self {
        Self {
            next_port: Mutex::new(first_port),
            requests: Arc::new(Mutex::new(Vec::new())),
            closes: Arc::new(AtomicUsize::new(0)),
        }
    }
}

struct StubForward {
    port: u16,
    closes: Arc<AtomicUsize>,
}

impl Forward for StubForward {
    fn local_port(&self) -> u16 {
        self.port
    }
}

impl Drop for StubForward {
    fn drop(&mut self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl ForwardOpener for StubOpener {
    async fn open(&self, config: SshTunnelConfig) -> Result<Arc<dyn Forward>, AdapterError> {
        let port = config.local_port.unwrap_or_else(|| {
            let mut next = self.next_port.lock().unwrap();
            let port = *next;
            *next += 1;
            port
        });
        self.requests.lock().unwrap().push(config);
        Ok(Arc::new(StubForward {
            port,
            closes: self.closes.clone(),
        }))
    }
}

fn stub_transport() -> Arc<StubOpener> {
    stub_transport_from(45_000)
}

fn stub_transport_from(first_port: u16) -> Arc<StubOpener> {
    let opener = Arc::new(StubOpener::new(first_port));
    set_test_forward_opener(opener.clone());
    opener
}

#[tokio::test]
async fn forward_serves_the_port_it_advertises() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _guard = SSH_TEST_LOCK.lock().await;
    reset_forwards_for_test();
    // Stands in for the far end of the tunnel: the port open_forward advertises
    // has to be the one the transport actually opened.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4];
        sock.read_exact(&mut buf).await.unwrap();
        sock.write_all(&buf).await.unwrap();
    });

    stub_transport_from(port);
    let conn = make_integration("ssh1", json!({"host":"bastion","user":"alice"}));
    let fwd = open_forward("owner1", &conn, "db.internal", 5432, None)
        .await
        .unwrap();
    assert_eq!(fwd.local_port, port);

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", fwd.local_port))
        .await
        .expect("the advertised port must accept connections");
    sock.write_all(b"ping").await.unwrap();
    let mut back = [0u8; 4];
    sock.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"ping");
    reset_forwards_for_test();
}

#[tokio::test]
async fn forward_asks_the_transport_for_the_remote_target() {
    let _guard = SSH_TEST_LOCK.lock().await;
    reset_forwards_for_test();
    let opener = stub_transport();
    let conn = make_integration(
        "ssh1",
        json!({"host":"bastion","port":2222,"user":"alice","auth_type":"key","key_path":"~/.ssh/id_ed25519"}),
    );
    open_forward("owner1", &conn, "db.internal", 5432, None)
        .await
        .unwrap();
    let requests = opener.requests.lock().unwrap();
    let req = requests.first().expect("the transport must be asked");
    assert_eq!(req.host, "bastion");
    assert_eq!(req.port, 2222);
    assert_eq!(req.user, "alice");
    assert_eq!(req.auth_type, "key");
    assert_eq!(req.remote_host, "db.internal");
    assert_eq!(req.remote_port, 5432);
    assert_eq!(req.local_port, None);
    drop(requests);
    reset_forwards_for_test();
}

#[tokio::test]
async fn closing_a_forward_tears_down_the_tunnel_and_frees_the_port() {
    let _guard = SSH_TEST_LOCK.lock().await;
    reset_forwards_for_test();
    let opener = stub_transport();
    let conn = make_integration("ssh1", json!({"host":"h"}));
    let fwd = open_forward("owner1", &conn, "localhost", 5432, Some(45_600))
        .await
        .unwrap();
    assert_eq!(opener.closes.load(Ordering::SeqCst), 0);

    assert!(close_forward("owner1", &conn, &fwd.id));
    assert_eq!(opener.closes.load(Ordering::SeqCst), 1);
    // The port is free again, so the same one can be asked for.
    let reopened = open_forward("owner1", &conn, "cache", 6379, Some(45_600))
        .await
        .unwrap();
    assert_eq!(reopened.local_port, 45_600);
    reset_forwards_for_test();
}

#[tokio::test]
async fn forward_idempotency_per_target() {
    let _guard = SSH_TEST_LOCK.lock().await;
    reset_forwards_for_test();
    stub_transport();
    let (_dir, _store) = temp_store();
    let conn = make_integration("ssh1", json!({"host":"h","port":22}));
    let owner = "owner1";
    let f1 = open_forward(owner, &conn, "localhost", 5432, None)
        .await
        .unwrap();
    let f2 = open_forward(owner, &conn, "localhost", 5432, None)
        .await
        .unwrap();
    assert_eq!(f1.local_port, f2.local_port);
    assert_eq!(f1.id, f2.id);
    // different target -> different port
    let f3 = open_forward(owner, &conn, "localhost", 6379, None)
        .await
        .unwrap();
    assert_ne!(f1.local_port, f3.local_port);
    // different remote_host same port -> different id
    let f4 = open_forward(owner, &conn, "db.internal", 5432, None)
        .await
        .unwrap();
    assert_ne!(f1.id, f4.id);
    reset_forwards_for_test();
}

#[tokio::test]
async fn close_unknown_id() {
    let _guard = SSH_TEST_LOCK.lock().await;
    reset_forwards_for_test();
    let (_dir, store) = temp_store();
    let conn =
        make_integration_with_policy("ssh1", json!({"host":"h"}), Some(ssh_all_enabled_policy()));
    // via tool
    let mut host = CaptureHost {
        tools: HashMap::new(),
    };
    register_ssh_server(&mut host, &conn, "owner1", store.clone()).unwrap();
    let handler = host.tools.get("close_forward").unwrap().1.clone();
    let res = handler(json!({"id":"localhost:9999"})).await;
    assert!(res.is_error);
    assert!(res.text().contains("No open forward"));
    // direct API
    assert!(!close_forward("owner1", &conn, "localhost:9999"));
    reset_forwards_for_test();
}

#[tokio::test]
async fn pending_approval_surfaced_as_pending() {
    let _guard = SSH_TEST_LOCK.lock().await;
    let (_dir, store) = temp_store();
    let conn = make_integration("ssh1", json!({"host":"example.com"}));
    let exec = Arc::new(StubExecutor {
        handler: Arc::new(|_cmd, _timeout| {
            Err(AdapterError::new("SSH connect is still running — authenticating, or waiting on an SSH agent or proxy approval. It continues in the background; retry in a moment. If it keeps repeating, check for a pending agent (e.g. 1Password) prompt.").with_code(crate::error::SSH_CONNECT_PENDING_CODE))
        }),
    });
    set_test_executor(exec);
    let mut host = CaptureHost {
        tools: HashMap::new(),
    };
    register_ssh_server(&mut host, &conn, "owner1", store.clone()).unwrap();
    let handler = host.tools.get("run_command").unwrap().1.clone();
    let res = handler(json!({"command":"ls"})).await;
    // Should be error but humanized as pending, not generic failure; and error hook suppressed (tested via gate)
    assert!(res.is_error);
    assert!(res.text().contains("waiting on an approval") || res.text().contains("still running"));
    clear_test_executor();
    reset_forwards_for_test();
}

#[tokio::test]
async fn list_forwards_and_close_flow() {
    let _guard = SSH_TEST_LOCK.lock().await;
    reset_forwards_for_test();
    stub_transport();
    let (_dir, store) = temp_store();
    let conn =
        make_integration_with_policy("ssh1", json!({"host":"h"}), Some(ssh_all_enabled_policy()));
    let owner = "owner1";
    let f = open_forward(owner, &conn, "localhost", 5432, Some(45500))
        .await
        .unwrap();
    assert_eq!(f.local_port, 45500);
    let list = list_forwards(owner, &conn);
    assert_eq!(list.len(), 1);
    // via tools list_forwards
    let mut host = CaptureHost {
        tools: HashMap::new(),
    };
    register_ssh_server(&mut host, &conn, owner, store.clone()).unwrap();
    let list_handler = host.tools.get("list_forwards").unwrap().1.clone();
    let res = list_handler(json!({})).await;
    assert!(!res.is_error);
    assert!(res.text().contains("localhost:5432"));
    // close via tool
    let close_handler = host.tools.get("close_forward").unwrap().1.clone();
    let res2 = close_handler(json!({"id": f.id.clone()})).await;
    assert!(!res2.is_error);
    assert_eq!(list_forwards(owner, &conn).len(), 0);
    reset_forwards_for_test();
}

#[tokio::test]
async fn local_port_already_in_use() {
    let _guard = SSH_TEST_LOCK.lock().await;
    reset_forwards_for_test();
    stub_transport();
    let conn = make_integration("ssh1", json!({"host":"h"}));
    let owner = "owner1";
    // occupy port by opening forward
    let _f1 = open_forward(owner, &conn, "localhost", 5432, Some(45600))
        .await
        .unwrap();
    let err = open_forward(owner, &conn, "localhost", 6379, Some(45600))
        .await
        .unwrap_err();
    assert!(err.message.contains("already in use"));
    reset_forwards_for_test();
}

#[tokio::test]
async fn saved_commands_only_projection_and_run() {
    let _guard = SSH_TEST_LOCK.lock().await;
    let (_dir, store) = temp_store();
    let conn =
        make_integration_with_policy("ssh1", json!({"host":"h"}), Some(ssh_all_enabled_policy()));
    store
        .create_saved_command(&pluk_store::SavedCommandInput {
            connection_id: conn.id.clone(),
            name: "logs".into(),
            command: "docker logs app".into(),
            working_dir: Some("/srv/app".into()),
        })
        .unwrap();
    store
        .create_saved_command(&pluk_store::SavedCommandInput {
            connection_id: conn.id.clone(),
            name: "ps".into(),
            command: "ps aux".into(),
            working_dir: None,
        })
        .unwrap();

    let mut host = CaptureHost {
        tools: HashMap::new(),
    };
    register_ssh_server(&mut host, &conn, "owner1", store.clone()).unwrap();

    // list without only -> default fields name,command
    let list_handler = host.tools.get("list_saved_commands").unwrap().1.clone();
    let res = list_handler(json!({})).await;
    assert!(!res.is_error);
    let val: Value = serde_json::from_str(res.text()).unwrap();
    if let Value::Array(arr) = val {
        for obj in arr {
            assert!(obj.get("name").is_some());
            assert!(obj.get("command").is_some());
        }
    } else {
        panic!("expected array");
    }

    // list with only location -> working_dir
    let res2 = list_handler(json!({"only":["location"]})).await;
    assert!(!res2.is_error);
    let _val2: Value = serde_json::from_str(res2.text()).unwrap();
    // should contain working_dir
    assert!(res2.text().contains("working_dir"));

    // run_saved_command via stub
    let exec = Arc::new(StubExecutor {
        handler: Arc::new(|cmd, _| {
            Ok(ExecResult {
                stdout: format!("ran {}", cmd),
                stderr: "".into(),
                code: Some(0),
                truncated: false,
            })
        }),
    });
    set_test_executor(exec);
    let run_handler = host.tools.get("run_saved_command").unwrap().1.clone();
    let res3 = run_handler(json!({"name":"ps"})).await;
    assert!(!res3.is_error);
    assert!(res3.text().contains("exit code"));

    // saved command not found
    let res4 = run_handler(json!({"name":"missing"})).await;
    assert!(res4.is_error);
    assert!(res4.text().contains("not found"));
    clear_test_executor();
    reset_forwards_for_test();
}

/// Saved commands are curated in the app, so they run outside the allowlist that
/// governs commands an agent writes.
#[tokio::test]
async fn saved_commands_run_outside_the_allowlist() {
    let _guard = SSH_TEST_LOCK.lock().await;
    let (_dir, store) = temp_store();
    let conn =
        make_integration_with_policy("ssh1", json!({"host":"h"}), Some(ssh_all_enabled_policy()));
    store
        .create_saved_command(&pluk_store::SavedCommandInput {
            connection_id: conn.id.clone(),
            name: "ping-api".into(),
            command: "curl https://example.com".into(),
            working_dir: None,
        })
        .unwrap();
    assert!(!evaluate_command("curl https://example.com").ok);

    let ran = record_commands();
    let mut host = CaptureHost {
        tools: HashMap::new(),
    };
    register_ssh_server(&mut host, &conn, "owner1", store.clone()).unwrap();
    let handler = host.tools.get("run_saved_command").unwrap().1.clone();
    let res = handler(json!({"name":"ping-api"})).await;
    assert!(!res.is_error, "got: {}", res.text());
    assert_eq!(ran.lock().unwrap().as_slice(), ["curl https://example.com"]);
    clear_test_executor();
    reset_forwards_for_test();
}

/// An executor that records what it was asked to run and reports success.
fn record_commands() -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    set_test_executor(Arc::new(StubExecutor {
        handler: Arc::new(move |cmd, _| {
            recorder.lock().unwrap().push(cmd.to_string());
            Ok(ExecResult {
                stdout: "ok".into(),
                stderr: String::new(),
                code: Some(0),
                truncated: false,
            })
        }),
    }));
    seen
}

#[tokio::test]
async fn working_dir_prefixes_an_allowed_command() {
    let _guard = SSH_TEST_LOCK.lock().await;
    let (_dir, store) = temp_store();
    let conn = make_integration("ssh1", json!({"host":"h"}));
    let ran = record_commands();
    let mut host = CaptureHost {
        tools: HashMap::new(),
    };
    register_ssh_server(&mut host, &conn, "owner1", store.clone()).unwrap();
    let handler = host.tools.get("run_command").unwrap().1.clone();

    let res = handler(json!({"command":"ls -la","working_dir":"/srv/app"})).await;
    assert!(!res.is_error, "got: {}", res.text());
    assert_eq!(ran.lock().unwrap().as_slice(), ["cd '/srv/app' && ls -la"]);
    clear_test_executor();
    reset_forwards_for_test();
}

#[tokio::test]
async fn working_dir_with_shell_characters_is_refused() {
    let _guard = SSH_TEST_LOCK.lock().await;
    let (_dir, store) = temp_store();
    let conn = make_integration("ssh1", json!({"host":"h"}));
    let ran = record_commands();
    let mut host = CaptureHost {
        tools: HashMap::new(),
    };
    register_ssh_server(&mut host, &conn, "owner1", store.clone()).unwrap();
    let handler = host.tools.get("run_command").unwrap().1.clone();

    let res = handler(json!({"command":"ls","working_dir":"/srv; rm -rf /"})).await;
    assert!(res.is_error);
    assert!(res.text().contains("working_dir not allowed"), "got: {}", res.text());
    assert!(ran.lock().unwrap().is_empty());
    clear_test_executor();
    reset_forwards_for_test();
}

#[test]
fn instructions_state_the_policy_that_is_enforced() {
    let conn = make_integration("ssh1", json!({"host":"h"}));
    let text = ssh_instructions(&conn);
    assert!(text.contains(&policy_summary()));
    assert!(
        !text.to_lowercase().contains("unrestricted"),
        "the allowlist refuses most commands: {text}"
    );
}
