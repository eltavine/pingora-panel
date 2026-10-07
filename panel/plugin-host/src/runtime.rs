//! The plugins the host runs (ADR 0044): each one's process, health and
//! calls in flight, restarted with a backoff when it fails.

use crate::process::{self, Launch, Process};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{mpsc, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;

/// How often a running plugin's health is checked.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(10);
/// How long a health check may take.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// Failed checks in a row that make a plugin degraded.
pub const FAILURES_TO_DEGRADE: u32 = 3;
const LEAST_BACKOFF: Duration = Duration::from_secs(1);
const MOST_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum State {
    Running,
    /// Failing its health checks, or exited; restarting.
    Degraded,
}

/// How a running plugin is doing.
#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    pub state: State,
    pub started_at: SystemTime,
    pub checked_at: Option<SystemTime>,
    /// Failed checks in a row.
    pub failures: u32,
    pub restarts: u32,
    pub error: Option<String>,
}

/// A change in a running plugin's health, recorded in the audit trail.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Change {
    Degraded {
        name: String,
        version: String,
        reason: String,
    },
    Recovered {
        name: String,
        version: String,
    },
}

/// Why a call is not made.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Refusal {
    Degraded(String),
    Busy(u32),
}

/// A plugin version the host runs.
pub struct Instance {
    pub launch: Launch,
    granted: BTreeSet<String>,
    calls: Arc<Semaphore>,
    channel: RwLock<Channel>,
    health: Mutex<Health>,
    stop: CancellationToken,
    monitor: Mutex<Option<JoinHandle<()>>>,
}

impl Instance {
    fn new(launch: Launch, channel: Channel) -> Self {
        Self {
            granted: launch.granted.iter().cloned().collect(),
            calls: Arc::new(Semaphore::new(launch.limits.concurrency.max(1) as usize)),
            channel: RwLock::new(channel),
            health: Mutex::new(Health {
                state: State::Running,
                started_at: SystemTime::now(),
                checked_at: Some(SystemTime::now()),
                failures: 0,
                restarts: 0,
                error: None,
            }),
            stop: CancellationToken::new(),
            monitor: Mutex::new(None),
            launch,
        }
    }

    pub fn name(&self) -> &str {
        &self.launch.manifest.name
    }

    pub fn version(&self) -> &str {
        &self.launch.manifest.version
    }

    pub fn provides(&self, port: &str) -> bool {
        self.launch
            .manifest
            .ports
            .iter()
            .any(|provided| provided == port)
    }

    pub fn is_granted(&self, capability: &str) -> bool {
        self.granted.contains(capability)
    }

    pub fn call_timeout(&self) -> Duration {
        Duration::from_millis(u64::from(self.launch.limits.call_timeout_ms))
    }

    pub fn health(&self) -> Health {
        self.health
            .lock()
            .expect("health is never poisoned")
            .clone()
    }

    /// A slot for one call and the channel to make it on.
    pub fn admit(&self) -> Result<(OwnedSemaphorePermit, Channel), Refusal> {
        let health = self.health();
        if health.state == State::Degraded {
            return Err(Refusal::Degraded(health.error.unwrap_or_default()));
        }
        let permit = Arc::clone(&self.calls)
            .try_acquire_owned()
            .map_err(|_| Refusal::Busy(self.launch.limits.concurrency))?;
        let channel = self
            .channel
            .read()
            .expect("the channel is never poisoned")
            .clone();
        Ok((permit, channel))
    }

    fn update<T>(&self, change: impl FnOnce(&mut Health) -> T) -> T {
        change(&mut self.health.lock().expect("health is never poisoned"))
    }

    fn degrade(&self, reason: String, changes: &mpsc::UnboundedSender<Change>) {
        let was = self.update(|health| {
            health.error = Some(reason.clone());
            std::mem::replace(&mut health.state, State::Degraded)
        });
        if was == State::Running {
            let _ = changes.send(Change::Degraded {
                name: self.name().to_owned(),
                version: self.version().to_owned(),
                reason,
            });
        }
    }

    fn healthy(&self, changes: &mpsc::UnboundedSender<Change>) {
        let was = self.update(|health| {
            health.checked_at = Some(SystemTime::now());
            health.failures = 0;
            health.error = None;
            std::mem::replace(&mut health.state, State::Running)
        });
        if was == State::Degraded {
            let _ = changes.send(Change::Recovered {
                name: self.name().to_owned(),
                version: self.version().to_owned(),
            });
        }
    }

    fn failed(&self, problem: String) -> u32 {
        self.update(|health| {
            health.checked_at = Some(SystemTime::now());
            health.failures += 1;
            health.error = Some(problem);
            health.failures
        })
    }
}

/// Every plugin the host runs, by name.
pub struct Runtime {
    instances: RwLock<HashMap<String, Arc<Instance>>>,
    changes: mpsc::UnboundedSender<Change>,
}

impl Runtime {
    /// The runtime, and where its health changes arrive.
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<Change>) {
        let (changes, received) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                instances: RwLock::new(HashMap::new()),
                changes,
            }),
            received,
        )
    }

    pub fn get(&self, name: &str) -> Option<Arc<Instance>> {
        self.instances
            .read()
            .expect("instances are never poisoned")
            .get(name)
            .cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .instances
            .read()
            .expect("instances are never poisoned")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Starts `launch` and, once it answers healthy with its settings
    /// applied, puts it in place of whatever ran for the plugin, which then
    /// stops. When it fails to start, what ran keeps running.
    pub async fn run(self: &Arc<Self>, launch: Launch) -> Result<(), String> {
        let process = Process::start(&launch).await?;
        let name = launch.manifest.name.clone();
        let instance = Arc::new(Instance::new(launch, process.channel.clone()));
        let monitor = tokio::spawn(monitor(
            self.changes.clone(),
            Arc::clone(&instance),
            process,
        ));
        *instance
            .monitor
            .lock()
            .expect("the monitor is never poisoned") = Some(monitor);
        let previous = self
            .instances
            .write()
            .expect("instances are never poisoned")
            .insert(name, instance);
        if let Some(previous) = previous {
            stop(&previous).await;
        }
        Ok(())
    }

    /// Stops the plugin's process; `false` when it did not run.
    pub async fn stop(&self, name: &str) -> bool {
        let removed = self
            .instances
            .write()
            .expect("instances are never poisoned")
            .remove(name);
        match removed {
            Some(instance) => {
                stop(&instance).await;
                true
            }
            None => false,
        }
    }

    /// Stops every plugin, as the host stops.
    pub async fn stop_all(&self) {
        for name in self.names() {
            self.stop(&name).await;
        }
    }
}

async fn stop(instance: &Instance) {
    instance.stop.cancel();
    let monitor = instance
        .monitor
        .lock()
        .expect("the monitor is never poisoned")
        .take();
    if let Some(monitor) = monitor {
        let _ = monitor.await;
    }
}

async fn monitor(
    changes: mpsc::UnboundedSender<Change>,
    instance: Arc<Instance>,
    mut process: Process,
) {
    let mut backoff = LEAST_BACKOFF;
    loop {
        tokio::select! {
            () = instance.stop.cancelled() => {
                process.stop().await;
                return;
            }
            exited = process.exited() => {
                let reason = match exited {
                    Ok(status) => format!("the plugin exited: {status}"),
                    Err(error) => format!("the plugin's process is lost: {error}"),
                };
                instance.degrade(reason, &changes);
            }
            () = tokio::time::sleep(CHECK_INTERVAL) => {
                let channel = instance.channel.read().expect("the channel is never poisoned").clone();
                match process::check(&channel, CHECK_TIMEOUT).await {
                    Ok(()) => {
                        instance.healthy(&changes);
                        continue;
                    }
                    Err(problem) => {
                        if instance.failed(problem.clone()) < FAILURES_TO_DEGRADE {
                            continue;
                        }
                        instance.degrade(problem, &changes);
                        process.stop().await;
                    }
                }
            }
        }
        match restart(&instance, &changes, &mut backoff).await {
            Some(restarted) => process = restarted,
            None => return,
        }
    }
}

/// Starts the instance's version again after a backoff, until it starts or
/// the plugin is stopped.
async fn restart(
    instance: &Instance,
    changes: &mpsc::UnboundedSender<Change>,
    backoff: &mut Duration,
) -> Option<Process> {
    loop {
        tokio::select! {
            () = instance.stop.cancelled() => return None,
            () = tokio::time::sleep(*backoff) => {}
        }
        match Process::start(&instance.launch).await {
            Ok(process) => {
                *instance
                    .channel
                    .write()
                    .expect("the channel is never poisoned") = process.channel.clone();
                instance.update(|health| health.restarts += 1);
                instance.healthy(changes);
                *backoff = LEAST_BACKOFF;
                return Some(process);
            }
            Err(problem) => {
                instance.update(|health| health.error = Some(problem));
                *backoff = (*backoff * 2).min(MOST_BACKOFF);
            }
        }
    }
}
