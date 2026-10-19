//! Warm connections: one tunnel and driver per integration and database,
//! shared by every call that needs it.
//!
//! A connect belongs to the pool, not to the call that asked for it. It runs
//! as its own task, so a caller that stops waiting never kills the `ssh`
//! process a person may be approving right now, and callers that arrive
//! meanwhile join the same connect instead of starting another.

use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use pluk_ssh::pending::CodedError;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::config::SqlConfig;
use crate::driver::Driver;
use crate::error::DriverError;
use crate::factory::{DriverWithTunnel, OwnedTunnel};

/// A connection nobody used for this long is closed.
const IDLE_EVICT: Duration = Duration::from_secs(5 * 60);
/// A connection idle for longer than this is checked before it is handed out.
const STALE_AFTER: Duration = Duration::from_secs(30);
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// Covers the SSH handshake, which waits for a person, plus the database login.
pub const SSH_CONNECT_BUDGET: Duration =
    Duration::from_millis(pluk_ssh::HANDSHAKE_TIMEOUT_MS + 15_000);
const DIRECT_CONNECT_BUDGET: Duration = Duration::from_secs(60);

type Built = Result<Arc<Pooled>, DriverError>;

/// What a call needs from the pool, derived from its connection settings.
#[derive(Clone)]
pub struct Target {
    key: String,
    fingerprint: u64,
    integration: String,
    group: Option<String>,
    /// Identifies the SSH master this connection rides on.
    master: Option<String>,
    pooled: bool,
    cfg: SqlConfig,
}

impl Target {
    pub fn new(cfg: &SqlConfig) -> Self {
        let integration = cfg.integration_id.clone().unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{cfg:?}").hash(&mut hasher);
        let master = (cfg.r#type != "sqlite" && cfg.is_use_ssh())
            .then_some(cfg.ssh_host.as_deref())
            .flatten()
            .map(|host| {
                format!(
                    "{host}:{}:{}",
                    cfg.ssh_port.unwrap_or(22),
                    cfg.ssh_user.as_deref().unwrap_or_default()
                )
            });
        Self {
            key: format!("{integration}::{}", cfg.database.as_deref().unwrap_or_default()),
            fingerprint: hasher.finish(),
            integration,
            group: cfg.via_group.clone(),
            master,
            pooled: cfg.r#type != "sqlite",
            cfg: cfg.clone(),
        }
    }

    pub fn uses_ssh(&self) -> bool {
        self.master.is_some()
    }

    fn budget(&self) -> Duration {
        if self.uses_ssh() {
            SSH_CONNECT_BUDGET
        } else {
            DIRECT_CONNECT_BUDGET
        }
    }

    fn budget_error(&self) -> DriverError {
        let seconds = self.budget().as_secs();
        DriverError::Connection(match self.cfg.ssh_host.as_deref().filter(|_| self.uses_ssh()) {
            Some(host) => format!(
                "The SSH tunnel to {host} didn't come up within {seconds}s. Approve any sign-in prompt, then retry. Retrying is safe."
            ),
            None => format!(
                "The database didn't answer within {seconds}s. Check your network and connection settings, then retry. Retrying is safe."
            ),
        })
    }
}

/// A live driver and the tunnel it runs over. Dropping the last handle closes both.
pub struct Pooled {
    driver: Arc<dyn Driver>,
    tunnel: Option<OwnedTunnel>,
    master: Option<String>,
    built_at: Instant,
    last_used: Mutex<Instant>,
    runtime: tokio::runtime::Handle,
}

impl Pooled {
    fn new(connected: DriverWithTunnel, target: &Target) -> Self {
        let now = Instant::now();
        Self {
            driver: Arc::from(connected.driver),
            tunnel: connected.tunnel,
            master: target.master.clone(),
            built_at: now,
            last_used: Mutex::new(now),
            runtime: tokio::runtime::Handle::current(),
        }
    }

    pub fn driver(&self) -> Arc<dyn Driver> {
        self.driver.clone()
    }

    fn idle(&self) -> Duration {
        self.last_used.lock().unwrap().elapsed()
    }

    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }
}

impl Drop for Pooled {
    fn drop(&mut self) {
        let driver = self.driver.clone();
        let tunnel = self.tunnel.take();
        // A drop can happen on a thread outside the runtime, such as a command handler.
        self.runtime.spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(1), driver.close()).await;
            drop(tunnel);
        });
    }
}

#[derive(Clone)]
struct Build {
    id: u64,
    done: watch::Receiver<Option<Built>>,
    abort: AbortHandle,
    /// When a waiter last got a "still connecting" answer, so a burst of
    /// parallel calls counts as one report.
    reported: Arc<Mutex<Option<Instant>>>,
}

enum State {
    Building(Build),
    Ready(Arc<Pooled>),
}

struct Slot {
    fingerprint: u64,
    integration: String,
    group: Option<String>,
    state: State,
}

impl Slot {
    fn retire(self) {
        if let State::Building(build) = self.state {
            build.abort.abort();
        }
    }
}

#[derive(Default)]
struct Inner {
    slots: Mutex<HashMap<String, Slot>>,
    /// When each SSH master was last killed for being broken.
    killed: Mutex<HashMap<String, Instant>>,
    next_build: AtomicU64,
}

pub enum CheckoutError {
    Failed(DriverError),
    /// The connect is still running. It carries on, and a retry finds the result.
    Waiting(CodedError),
}

enum Claim {
    Ready(Arc<Pooled>),
    Connecting(Build),
}

#[derive(Clone, Default)]
pub struct DriverPool(Arc<Inner>);

impl DriverPool {
    /// Hand out the connection for `target`, connecting first when there is none.
    ///
    /// `wait` bounds how long an SSH connect is waited on; other connects are
    /// waited on until they settle. `connect` runs only when this call starts
    /// the connect.
    pub async fn checkout<F, Fut>(
        &self,
        target: &Target,
        wait: Duration,
        connect: F,
    ) -> Result<Arc<Pooled>, CheckoutError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<DriverWithTunnel, DriverError>> + Send + 'static,
    {
        if !target.pooled {
            let connected = connect().await.map_err(CheckoutError::Failed)?;
            return Ok(Arc::new(Pooled::new(connected, target)));
        }
        let mut connect = Some(connect);
        loop {
            match self.claim(target, &mut connect) {
                Claim::Ready(pooled) => {
                    if is_healthy(&pooled).await {
                        pooled.touch();
                        return Ok(pooled);
                    }
                    self.drop_broken(&pooled, &target.cfg).await;
                }
                Claim::Connecting(build) => return self.wait_for(target, build, wait).await,
            }
        }
    }

    fn claim<F, Fut>(&self, target: &Target, connect: &mut Option<F>) -> Claim
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<DriverWithTunnel, DriverError>> + Send + 'static,
    {
        let mut slots = self.0.slots.lock().unwrap();
        if let Some(slot) = slots.get(&target.key)
            && slot.fingerprint == target.fingerprint
        {
            return match &slot.state {
                State::Ready(pooled) => Claim::Ready(pooled.clone()),
                State::Building(build) => Claim::Connecting(build.clone()),
            };
        }
        // Settings changed since this connection was made.
        if let Some(stale) = slots.remove(&target.key) {
            stale.retire();
        }
        let connect = connect
            .take()
            .expect("a connect is only started once per checkout");
        let build = self.start_build(target, connect());
        slots.insert(
            target.key.clone(),
            Slot {
                fingerprint: target.fingerprint,
                integration: target.integration.clone(),
                group: target.group.clone(),
                state: State::Building(build.clone()),
            },
        );
        Claim::Connecting(build)
    }

    fn start_build<Fut>(&self, target: &Target, connect: Fut) -> Build
    where
        Fut: Future<Output = Result<DriverWithTunnel, DriverError>> + Send + 'static,
    {
        let id = self.0.next_build.fetch_add(1, Ordering::Relaxed);
        let (send, done) = watch::channel(None);
        let budget = target.budget();
        let work = tokio::spawn(async move { tokio::time::timeout(budget, connect).await });
        let abort = work.abort_handle();
        if target.uses_ssh() {
            pluk_ssh::start_connect_attempt(&target.key);
        }

        let pool = self.clone();
        let target = target.clone();
        tokio::spawn(async move {
            let built = match work.await {
                Ok(Ok(Ok(connected))) => Ok(Arc::new(Pooled::new(connected, &target))),
                Ok(Ok(Err(error))) => Err(error),
                Ok(Err(_elapsed)) => Err(target.budget_error()),
                Err(_) => Err(DriverError::Connection(
                    "The connection attempt was interrupted. Retry.".into(),
                )),
            };
            if target.uses_ssh() {
                pluk_ssh::clear_connect_episode(&target.key);
            }
            pool.settle(&target.key, id, &built);
            let _ = send.send(Some(built));
        });
        Build {
            id,
            done,
            abort,
            reported: Arc::default(),
        }
    }

    /// Record a connect's result, unless the entry was evicted or replaced meanwhile.
    fn settle(&self, key: &str, id: u64, built: &Built) {
        let mut slots = self.0.slots.lock().unwrap();
        let Some(slot) = slots.get_mut(key) else {
            return;
        };
        if !matches!(&slot.state, State::Building(build) if build.id == id) {
            return;
        }
        match built {
            Ok(pooled) => {
                slot.state = State::Ready(pooled.clone());
                self.evict_when_idle(key.to_string(), Arc::downgrade(pooled));
            }
            Err(_) => {
                slots.remove(key);
            }
        }
    }

    fn evict_when_idle(&self, key: String, pooled: Weak<Pooled>) {
        let pool = self.clone();
        tokio::spawn(async move {
            loop {
                let Some(live) = pooled.upgrade() else {
                    return;
                };
                let idle = live.idle();
                if idle >= IDLE_EVICT {
                    pool.discard_if(&key, &live);
                    return;
                }
                drop(live);
                tokio::time::sleep(IDLE_EVICT - idle).await;
            }
        });
    }

    async fn wait_for(
        &self,
        target: &Target,
        build: Build,
        wait: Duration,
    ) -> Result<Arc<Pooled>, CheckoutError> {
        let settled = settled(build.done.clone());
        let built = if target.uses_ssh() {
            match tokio::time::timeout(wait, settled).await {
                Ok(built) => built,
                Err(_) => return Err(CheckoutError::Waiting(self.still_connecting(target, &build))),
            }
        } else {
            settled.await
        };
        built.map_err(CheckoutError::Failed)
    }

    fn still_connecting(&self, target: &Target, build: &Build) -> CodedError {
        let agent = target
            .cfg
            .ssh_host
            .as_deref()
            .and_then(pluk_ssh::connect_agent);
        let coded = {
            let mut reported = build.reported.lock().unwrap();
            let fresh = reported.is_some_and(|at| {
                at.elapsed() < Duration::from_millis(pluk_ssh::SSH_CONNECT_WAIT_MS / 2)
            });
            if fresh {
                pluk_ssh::ssh_pending_error(agent)
            } else {
                *reported = Some(Instant::now());
                pluk_ssh::connect_wait_error(&target.key, agent)
            }
        };
        // A connect nobody approved is dropped, so the next call starts a fresh one.
        if pluk_ssh::is_ssh_stalled(Some(coded.code)) {
            let mut slots = self.0.slots.lock().unwrap();
            if matches!(slots.get(&target.key), Some(Slot { state: State::Building(b), .. }) if b.id == build.id)
                && let Some(slot) = slots.remove(&target.key)
            {
                slot.retire();
            }
        }
        coded
    }

    /// Forget a connection that failed, so the next call connects again.
    pub fn discard(&self, pooled: &Arc<Pooled>) {
        let key = {
            let slots = self.0.slots.lock().unwrap();
            slots
                .iter()
                .find(|(_, slot)| matches!(&slot.state, State::Ready(p) if Arc::ptr_eq(p, pooled)))
                .map(|(key, _)| key.clone())
        };
        if let Some(key) = key {
            self.discard_if(&key, pooled);
        }
    }

    fn discard_if(&self, key: &str, pooled: &Arc<Pooled>) {
        let mut slots = self.0.slots.lock().unwrap();
        if matches!(slots.get(key), Some(Slot { state: State::Ready(p), .. }) if Arc::ptr_eq(p, pooled))
        {
            slots.remove(key);
        }
    }

    /// Forget a broken connection and its SSH master, which a dead network can
    /// leave answering `ssh -O check` while carrying nothing. Parallel calls
    /// that find the same fault kill the master once, so the connection that
    /// replaces it is not killed in turn.
    pub async fn drop_broken(&self, pooled: &Arc<Pooled>, cfg: &SqlConfig) {
        self.discard(pooled);
        let Some(master) = &pooled.master else {
            return;
        };
        {
            let mut killed = self.0.killed.lock().unwrap();
            if killed.get(master).is_some_and(|at| *at > pooled.built_at) {
                return;
            }
            killed.insert(master.clone(), Instant::now());
        }
        crate::ssh_provider::force_reconnect(cfg).await;
    }

    /// Close every connection of an integration, or of any integration in a group.
    pub fn evict(&self, id: &str) {
        let removed: Vec<Slot> = {
            let mut slots = self.0.slots.lock().unwrap();
            let keys: Vec<String> = slots
                .iter()
                .filter(|(_, slot)| slot.integration == id || slot.group.as_deref() == Some(id))
                .map(|(key, _)| key.clone())
                .collect();
            keys.iter().filter_map(|key| slots.remove(key)).collect()
        };
        for slot in removed {
            slot.retire();
        }
    }
}

async fn is_healthy(pooled: &Pooled) -> bool {
    if pooled.idle() <= STALE_AFTER {
        return true;
    }
    matches!(
        tokio::time::timeout(HEALTHCHECK_TIMEOUT, pooled.driver.test_connection()).await,
        Ok(Ok(()))
    )
}

async fn settled(mut done: watch::Receiver<Option<Built>>) -> Built {
    match done.wait_for(Option::is_some).await {
        Ok(result) => result.clone().expect("waited for a result"),
        Err(_) => Err(DriverError::Connection(
            "The connection attempt was interrupted. Retry.".into(),
        )),
    }
}
