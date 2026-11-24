//! What the web app is given by its host and what it keeps while it runs.
//!
//! There is no global state and no path is built from `~`: the folders, the config, the key store, the clock, the way ProPresenter is
//! reached and the StagePilot installation all come in through [`WebStateBuilder`]. A [`WebState`] is cheap to clone (it is an `Arc`) and
//! may live inside a Tokio runtime owned by the host; the pipeline's blocking calls run on that runtime's blocking pool.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use indexmap::IndexMap;
use serde_json::Value;
use versevision_bible::{BibleSources, FileKeyStore, InstallationAuth, KeyStore, RegistryBuilder};
use versevision_core::bibles::{
    BibleError, BiblePassages, InMemoryBible, ResolvedPassage, VerseCache,
};
use versevision_core::config::AppConfig;
use versevision_core::notes::TraceSink;
use versevision_core::references::PassageReference;
use versevision_pp::clock::{Clock, SystemClock};
use versevision_pp::dirs::{Dirs, DirsEnv};
use versevision_pp::history::HistoryStore;
use versevision_pp::pipeline::{Planned, SharedUuids, Target, Timing};
use versevision_pp::process::SystemProbe;
use versevision_pp::transport::{LocalTransport, Transport};

/// Makes the transport to the computer that runs ProPresenter for one piece of work. It is called with the configuration that work
/// uses, so a host (or a test) can hand out a fake. The default is [`LocalTransport`].
pub type TransportFactory = Arc<dyn Fn(&AppConfig) -> Box<dyn Transport> + Send + Sync>;

/// Makes the Bible text source for one piece of work, from the configuration that work uses (so `bible.source` and the providers
/// follow what the Config page saved). The default builds [`BibleSources`] from the key store, the cache and the installation.
pub type BibleFactory = Arc<dyn Fn(&AppConfig) -> BibleHandle + Send + Sync>;

/// A Bible text source the planner reads.
pub enum BibleHandle {
    /// The providers of `[bible]` and the StagePilot service, following `bible.source`.
    Sources(Box<BibleSources>),
    /// Verse text from memory (tests, offline use).
    Memory(InMemoryBible),
}

impl BiblePassages for BibleHandle {
    async fn fetch(
        &self,
        reference: &PassageReference,
        trace: &mut dyn TraceSink,
    ) -> Result<ResolvedPassage, BibleError> {
        match self {
            BibleHandle::Sources(sources) => sources.fetch(reference, trace).await,
            BibleHandle::Memory(memory) => memory.fetch(reference, trace).await,
        }
    }

    fn description(&self) -> String {
        match self {
            BibleHandle::Sources(sources) => sources.description(),
            BibleHandle::Memory(memory) => memory.description(),
        }
    }
}

/// Start at login (a launch agent, a systemd user unit, a scheduled task) belongs to whatever installs the app, so the page asks
/// the host. Every method returns the status as the page reads it (`supported`, `enabled`, `platform`, `detail`, `host`, `port`).
pub trait Autostart: Send + Sync {
    fn status(&self) -> Result<Value, String>;
    fn enable(&self, host: &str, port: u16) -> Result<Value, String>;
    fn disable(&self) -> Result<Value, String>;
}

/// `Job` of Python's server: one planning or importing run, polled by the page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobKind {
    Plan,
    Apply,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Plan => "plan",
            JobKind::Apply => "apply",
        }
    }
}

/// What a finished review keeps for the import after it.
pub struct Reviewed {
    pub text: String,
    pub plan: Arc<Planned>,
    pub target: Option<Arc<Target>>,
}

pub struct Job {
    pub id: String,
    pub kind: JobKind,
    /// `running` | `done` | `error`
    pub state: &'static str,
    pub message: String,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub exit_code: i32,
    pub name: String,
    pub mode: String,
    pub title: String,
    pub title_override: Option<String>,
    pub library: Option<String>,
    pub version: Option<String>,
    pub target_arg: Option<String>,
    pub profile: Option<String>,
    pub verse_look: Option<String>,
    pub point_look: Option<String>,
    pub run_id: String,
    pub reviewed: Option<Reviewed>,
}

impl Job {
    pub fn new(kind: JobKind) -> Job {
        Job {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            kind,
            state: "running",
            message: "Starting".into(),
            result: None,
            error: None,
            exit_code: 0,
            name: String::new(),
            mode: "new".into(),
            title: String::new(),
            title_override: None,
            library: None,
            version: None,
            target_arg: None,
            profile: None,
            verse_look: None,
            point_look: None,
            run_id: String::new(),
            reviewed: None,
        }
    }
}

pub(crate) type SharedJob = Arc<Mutex<Job>>;

/// The jobs and the one-import-at-a-time flag, under one lock (`state.lock` of Python).
pub(crate) struct Control {
    pub jobs: IndexMap<String, SharedJob>,
    pub busy: bool,
}

pub(crate) struct Inner {
    pub dirs: Dirs,
    pub config: Mutex<AppConfig>,
    pub control: Mutex<Control>,
    pub token: Option<String>,
    pub app_version: String,
    pub clock: Arc<dyn Clock>,
    pub uuids: SharedUuids,
    pub timing: Timing,
    pub transport: TransportFactory,
    pub bible: BibleFactory,
    pub keys: Arc<dyn KeyStore>,
    pub auth: Option<Arc<dyn InstallationAuth>>,
    pub autostart: Option<Arc<dyn Autostart>>,
    pub history: HistoryStore,
    pub looks: Mutex<Option<(Instant, Vec<Value>)>>,
    pub listen: Mutex<Option<SocketAddr>>,
    pub allowed_hosts: Mutex<Option<Vec<String>>>,
}

/// The web app's state: hand it to [`crate::router`].
#[derive(Clone)]
pub struct WebState {
    pub(crate) inner: Arc<Inner>,
}

impl WebState {
    pub fn builder(dirs: Dirs, config: AppConfig) -> WebStateBuilder {
        WebStateBuilder {
            dirs,
            config,
            token: None,
            app_version: versevision_core::VERSEVISION_VERSION.to_string(),
            clock: None,
            uuids: None,
            timing: Timing::default(),
            home: None,
            transport: None,
            bible: None,
            keys: None,
            cache: None,
            auth: None,
            autostart: None,
            allowed_hosts: None,
        }
    }

    /// The configuration the next piece of work uses.
    pub fn config(&self) -> AppConfig {
        self.inner
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn dirs(&self) -> &Dirs {
        &self.inner.dirs
    }

    /// The address the page is served on, when the host serves it itself (see [`crate::serve`]); the start-at-login switch is only
    /// allowed for a loopback address.
    pub fn set_listen_addr(&self, addr: SocketAddr) {
        *self
            .inner
            .listen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(addr);
    }

    /// Only answer requests whose `Host` is one of these names (without the port), against DNS rebinding. [`crate::serve`] sets the
    /// loopback names when it binds a loopback address; a host that mounts the router decides for itself.
    pub fn set_allowed_hosts(&self, hosts: Option<Vec<String>>) {
        *self
            .inner
            .allowed_hosts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hosts;
    }

    pub(crate) fn has_token(&self) -> bool {
        self.inner.token.is_some()
    }

    /// Is any import (or undo) running?
    pub fn busy(&self) -> bool {
        self.inner
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .busy
    }
}

pub struct WebStateBuilder {
    dirs: Dirs,
    config: AppConfig,
    token: Option<String>,
    app_version: String,
    clock: Option<Arc<dyn Clock>>,
    uuids: Option<SharedUuids>,
    timing: Timing,
    home: Option<PathBuf>,
    transport: Option<TransportFactory>,
    bible: Option<BibleFactory>,
    keys: Option<Arc<dyn KeyStore>>,
    cache: Option<Arc<dyn VerseCache>>,
    auth: Option<Arc<dyn InstallationAuth>>,
    autostart: Option<Arc<dyn Autostart>>,
    allowed_hosts: Option<Vec<String>>,
}

impl WebStateBuilder {
    /// The access token a page that is not on loopback must present (`X-VerseVision-Token` or `?token=`).
    pub fn token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|t| !t.is_empty());
        self
    }
    /// What `/api/status` calls the version and what the run log records.
    pub fn app_version(mut self, version: &str) -> Self {
        self.app_version = version.to_string();
        self
    }
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }
    pub fn uuids(mut self, uuids: SharedUuids) -> Self {
        self.uuids = Some(uuids);
        self
    }
    pub fn timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self
    }
    /// The user's home folder, for `~` in the config's paths (default: the process's own).
    pub fn home(mut self, home: PathBuf) -> Self {
        self.home = Some(home);
        self
    }
    pub fn transport(mut self, factory: TransportFactory) -> Self {
        self.transport = Some(factory);
        self
    }
    pub fn bible(mut self, factory: BibleFactory) -> Self {
        self.bible = Some(factory);
        self
    }
    /// Where the Bible keys are kept (default: `keys.env` in the keys folder).
    pub fn keys(mut self, keys: Arc<dyn KeyStore>) -> Self {
        self.keys = Some(keys);
        self
    }
    /// The scripture cache the default Bible source uses (default: none).
    pub fn cache(mut self, cache: Arc<dyn VerseCache>) -> Self {
        self.cache = Some(cache);
        self
    }
    /// The StagePilot installation the Bible service is asked on behalf of; without it the service is reported as not available.
    pub fn installation_auth(mut self, auth: Arc<dyn InstallationAuth>) -> Self {
        self.auth = Some(auth);
        self
    }
    pub fn autostart(mut self, autostart: Arc<dyn Autostart>) -> Self {
        self.autostart = Some(autostart);
        self
    }
    pub fn allowed_hosts(mut self, hosts: Vec<String>) -> Self {
        self.allowed_hosts = Some(hosts);
        self
    }

    pub fn build(self) -> WebState {
        let clock: Arc<dyn Clock> = self.clock.unwrap_or_else(|| Arc::new(SystemClock::new()));
        let keys: Arc<dyn KeyStore> = self
            .keys
            .unwrap_or_else(|| Arc::new(FileKeyStore::new(self.dirs.keys_file())));
        let home = self.home.unwrap_or_else(|| DirsEnv::from_process().home);
        let transport: TransportFactory = self.transport.unwrap_or_else(|| {
            let clock = clock.clone();
            Arc::new(move |config: &AppConfig| {
                Box::new(LocalTransport::new(
                    &config.propresenter,
                    &config.node,
                    clock.clone(),
                    Arc::new(SystemProbe),
                    home.clone(),
                ))
            })
        });
        let bible: BibleFactory = self.bible.unwrap_or_else(|| {
            let keys = keys.clone();
            let cache = self.cache.clone();
            let auth = self.auth.clone();
            Arc::new(move |config: &AppConfig| {
                let mut builder = RegistryBuilder::new(keys.clone());
                if let Some(cache) = &cache {
                    builder = builder.cache(cache.clone());
                }
                if let Some(auth) = &auth {
                    builder = builder.installation_auth(auth.clone());
                }
                BibleHandle::Sources(Box::new(builder.build_sources(&config.bible)))
            })
        });
        let history = HistoryStore::new(self.dirs.history_path(), clock.clone());
        WebState {
            inner: Arc::new(Inner {
                history,
                dirs: self.dirs,
                config: Mutex::new(self.config),
                control: Mutex::new(Control {
                    jobs: IndexMap::new(),
                    busy: false,
                }),
                token: self.token,
                app_version: self.app_version,
                uuids: self.uuids.unwrap_or_else(SharedUuids::random),
                clock,
                timing: self.timing,
                transport,
                bible,
                keys,
                auth: self.auth,
                autostart: self.autostart,
                looks: Mutex::new(None),
                listen: Mutex::new(None),
                allowed_hosts: Mutex::new(self.allowed_hosts),
            }),
        }
    }
}
