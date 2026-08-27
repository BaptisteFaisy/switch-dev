//! Provider USB Windows pour une instance WebDriverAgent deja operationnelle.
//!
//! Ce module ne construit, ne signe, n'installe et ne lance pas WebDriverAgent
//! sur l'iPhone. Chaque appareil doit deja executer une version provisionnee de
//! WDA qui ecoute sur le port distant configure (8100 par defaut). Le seul
//! processus gere ici est un tunnel loopback `iproxy` distinct par UDID.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::blocking::{Client, Response};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::env;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_WDA_REMOTE_PORT: u16 = 8100;
const DEFAULT_LOCAL_PORT_START: u16 = 42_000;
const DEFAULT_LOCAL_PORT_END: u16 = 42_999;
const DEFAULT_PROBE_CACHE_TTL: Duration = Duration::from_secs(7);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_SESSION_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_TUNNEL_START_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_JSON_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;
const MAX_SCREENSHOT_RESPONSE_BYTES: usize = (MAX_SCREENSHOT_BYTES * 4 / 3) + (64 * 1024);
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_BUNDLE_ID_BYTES: usize = 255;
const MAX_UDID_BYTES: usize = 128;
const MAX_SESSION_ID_BYTES: usize = 256;
const MAX_GESTURE_DURATION_MS: u32 = 60_000;
const MAX_COORDINATE: u32 = 100_000;
const MAX_PORT_ATTEMPTS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IosWdaErrorKind {
    InvalidInput,
    Unavailable,
    Timeout,
    Protocol,
    InvalidSession,
    AmbiguousMutation,
    ResponseTooLarge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IosWdaError {
    kind: IosWdaErrorKind,
    message: String,
}

impl IosWdaError {
    fn new(kind: IosWdaErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> IosWdaErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for IosWdaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for IosWdaError {}

#[derive(Clone, Debug)]
pub struct IosWdaConfig {
    pub iproxy_program: PathBuf,
    pub remote_port: u16,
    pub local_port_start: u16,
    pub local_port_end: u16,
    pub probe_cache_ttl: Duration,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub session_timeout: Duration,
    pub screenshot_timeout: Duration,
    pub tunnel_start_timeout: Duration,
}

impl Default for IosWdaConfig {
    fn default() -> Self {
        Self {
            iproxy_program: PathBuf::from("iproxy"),
            remote_port: DEFAULT_WDA_REMOTE_PORT,
            local_port_start: DEFAULT_LOCAL_PORT_START,
            local_port_end: DEFAULT_LOCAL_PORT_END,
            probe_cache_ttl: DEFAULT_PROBE_CACHE_TTL,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            session_timeout: DEFAULT_SESSION_TIMEOUT,
            screenshot_timeout: DEFAULT_SCREENSHOT_TIMEOUT,
            tunnel_start_timeout: DEFAULT_TUNNEL_START_TIMEOUT,
        }
    }
}

impl IosWdaConfig {
    /// Charge uniquement des valeurs bornables. Aucun endpoint HTTP externe ne
    /// peut etre fourni par l'environnement : le client reste sur loopback.
    pub fn from_env() -> Result<Self, IosWdaError> {
        let mut config = Self::default();
        if let Some(program) = non_empty_env("CST_IPROXY_PATH") {
            config.iproxy_program = PathBuf::from(program);
        }
        config.remote_port = env_u16("CST_IOS_WDA_REMOTE_PORT", config.remote_port)?;
        config.local_port_start = env_u16("CST_IOS_WDA_LOCAL_PORT_START", config.local_port_start)?;
        config.local_port_end = env_u16("CST_IOS_WDA_LOCAL_PORT_END", config.local_port_end)?;
        config.probe_cache_ttl = Duration::from_millis(env_u64(
            "CST_IOS_WDA_PROBE_CACHE_MS",
            config.probe_cache_ttl.as_millis() as u64,
        )?);
        config.connect_timeout = Duration::from_millis(env_u64(
            "CST_IOS_WDA_CONNECT_TIMEOUT_MS",
            config.connect_timeout.as_millis() as u64,
        )?);
        config.request_timeout = Duration::from_millis(env_u64(
            "CST_IOS_WDA_REQUEST_TIMEOUT_MS",
            config.request_timeout.as_millis() as u64,
        )?);
        config.session_timeout = Duration::from_millis(env_u64(
            "CST_IOS_WDA_SESSION_TIMEOUT_MS",
            config.session_timeout.as_millis() as u64,
        )?);
        config.screenshot_timeout = Duration::from_millis(env_u64(
            "CST_IOS_WDA_SCREENSHOT_TIMEOUT_MS",
            config.screenshot_timeout.as_millis() as u64,
        )?);
        config.tunnel_start_timeout = Duration::from_millis(env_u64(
            "CST_IOS_WDA_TUNNEL_START_TIMEOUT_MS",
            config.tunnel_start_timeout.as_millis() as u64,
        )?);
        validate_config(&config)?;
        Ok(config)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IosWdaCapability {
    Tap,
    Swipe,
    TypeText,
    KeyEvent,
    OpenApp,
    Screenshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosWdaScreen {
    pub width: f64,
    pub height: f64,
    pub scale: Option<f64>,
    pub status_bar_height: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosWdaProbe {
    pub ready: bool,
    pub capabilities: Vec<IosWdaCapability>,
    pub screen: Option<IosWdaScreen>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IosWdaAction {
    Tap {
        x: u32,
        y: u32,
    },
    Swipe {
        start_x: u32,
        start_y: u32,
        end_x: u32,
        end_y: u32,
        duration_ms: u32,
    },
    TypeText {
        text: String,
    },
    KeyEvent {
        key: String,
    },
    OpenApp {
        bundle_id: String,
    },
    Screenshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IosWdaScreenshot {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IosWdaActionResult {
    pub detail: String,
    pub screenshot: Option<IosWdaScreenshot>,
}

#[derive(Clone)]
pub struct IosWdaProvider {
    inner: Arc<ProviderInner>,
}

impl IosWdaProvider {
    /// Construit le provider USB reel. WDA doit deja etre provisionne et lance
    /// sur chaque appareil ; cette API ne gere pas son cycle de signature.
    pub fn new(config: IosWdaConfig) -> Result<Self, IosWdaError> {
        validate_config(&config)?;
        Self::build(config.clone(), TunnelMode::Iproxy(config))
    }

    pub fn from_env() -> Result<Self, IosWdaError> {
        Self::new(IosWdaConfig::from_env()?)
    }

    /// Constructeur reserve aux tests/integrations locales. Chaque endpoint est
    /// refuse s'il ne pointe pas vers une adresse IP loopback.
    #[doc(hidden)]
    pub fn with_loopback_endpoints_for_tests(
        endpoints: HashMap<String, SocketAddr>,
    ) -> Result<Self, IosWdaError> {
        if endpoints.is_empty() {
            return Err(IosWdaError::new(
                IosWdaErrorKind::InvalidInput,
                "au moins un endpoint WDA de test est requis",
            ));
        }
        let mut seen = HashSet::new();
        for (udid, endpoint) in &endpoints {
            validate_udid(udid)?;
            validate_loopback(*endpoint)?;
            if !seen.insert(*endpoint) {
                return Err(IosWdaError::new(
                    IosWdaErrorKind::InvalidInput,
                    "un endpoint WDA de test ne peut appartenir qu'a un UDID",
                ));
            }
        }
        let config = IosWdaConfig::default();
        Self::build(config, TunnelMode::Fixed(endpoints))
    }

    #[doc(hidden)]
    pub fn with_loopback_endpoint_for_tests(
        udid: impl Into<String>,
        endpoint: SocketAddr,
    ) -> Result<Self, IosWdaError> {
        Self::with_loopback_endpoints_for_tests(HashMap::from([(udid.into(), endpoint)]))
    }

    fn build(config: IosWdaConfig, tunnel_mode: TunnelMode) -> Result<Self, IosWdaError> {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .user_agent("codex-switch-terminal-ios-wda/1")
            .build()
            .map_err(|_| {
                IosWdaError::new(
                    IosWdaErrorKind::Unavailable,
                    "impossible de construire le client WDA local",
                )
            })?;
        Ok(Self {
            inner: Arc::new(ProviderInner {
                client,
                tunnels: TunnelRegistry::new(tunnel_mode),
                states: Mutex::new(HashMap::new()),
                config,
            }),
        })
    }

    pub fn probe(&self, udid: &str) -> Result<IosWdaProbe, IosWdaError> {
        validate_udid(udid)?;
        let state = self.inner.device_state(udid)?;
        let mut state = lock(&state, "etat WDA par appareil")?;

        if let Some(cached) = &state.probe {
            if cached.at.elapsed() <= self.inner.config.probe_cache_ttl
                && self.inner.tunnels.is_alive(udid)
            {
                return Ok(cached.value.clone());
            }
        }

        for attempt in 0..2 {
            match self.inner.probe_once(udid, &mut state) {
                Ok(probe) => return Ok(probe),
                Err(error) if attempt == 0 && error.kind() == IosWdaErrorKind::InvalidSession => {
                    state.clear_session();
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("la boucle de probe WDA retourne toujours")
    }

    pub fn execute(
        &self,
        udid: &str,
        action: IosWdaAction,
    ) -> Result<IosWdaActionResult, IosWdaError> {
        validate_udid(udid)?;
        validate_action(&action)?;
        if action == IosWdaAction::Screenshot {
            return self.execute_screenshot(udid);
        }

        let state = self.inner.device_state(udid)?;
        let mut state = lock(&state, "etat WDA par appareil")?;
        let ready = self.inner.ensure_ready(udid, &mut state)?;
        let (path, payload, detail) = mutation_request(&ready.session_id, &action)?;
        let result = self.inner.request_json(
            ready.tunnel.endpoint,
            Method::POST,
            &path,
            Some(&payload),
            self.inner.config.request_timeout,
            RequestSafety::Mutation,
            MAX_JSON_RESPONSE_BYTES,
        );
        match result {
            Ok(_) => Ok(IosWdaActionResult {
                detail: detail.to_string(),
                screenshot: None,
            }),
            Err(error) => {
                if matches!(
                    error.kind(),
                    IosWdaErrorKind::InvalidSession
                        | IosWdaErrorKind::AmbiguousMutation
                        | IosWdaErrorKind::Unavailable
                        | IosWdaErrorKind::Timeout
                ) {
                    state.clear_session();
                }
                if matches!(
                    error.kind(),
                    IosWdaErrorKind::AmbiguousMutation | IosWdaErrorKind::Unavailable
                ) {
                    self.inner.tunnels.invalidate(udid);
                }
                // Une mutation n'est jamais rejouee : une rupture apres envoi
                // laisse son resultat sur l'appareil fondamentalement ambigu.
                Err(error)
            }
        }
    }

    pub fn invalidate(&self, udid: &str) {
        if let Ok(mut states) = self.inner.states.lock() {
            states.remove(udid);
        }
        self.inner.tunnels.invalidate(udid);
    }

    fn execute_screenshot(&self, udid: &str) -> Result<IosWdaActionResult, IosWdaError> {
        let state = self.inner.device_state(udid)?;
        let mut state = lock(&state, "etat WDA par appareil")?;
        for attempt in 0..2 {
            let ready = self.inner.ensure_ready(udid, &mut state)?;
            let response = self.inner.request_json(
                ready.tunnel.endpoint,
                Method::GET,
                "/screenshot",
                None,
                self.inner.config.screenshot_timeout,
                RequestSafety::Safe,
                MAX_SCREENSHOT_RESPONSE_BYTES,
            );
            match response.and_then(|value| decode_screenshot(&value)) {
                Ok(screenshot) => {
                    return Ok(IosWdaActionResult {
                        detail: "capture WDA terminee".to_string(),
                        screenshot: Some(screenshot),
                    })
                }
                Err(error) if attempt == 0 && error.kind() == IosWdaErrorKind::InvalidSession => {
                    state.clear_session();
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("la boucle de capture WDA retourne toujours")
    }
}

struct ProviderInner {
    client: Client,
    tunnels: TunnelRegistry,
    states: Mutex<HashMap<String, Arc<Mutex<DeviceState>>>>,
    config: IosWdaConfig,
}

impl ProviderInner {
    fn device_state(&self, udid: &str) -> Result<Arc<Mutex<DeviceState>>, IosWdaError> {
        let mut states = lock(&self.states, "registre des etats WDA")?;
        Ok(states
            .entry(udid.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(DeviceState::default())))
            .clone())
    }

    fn probe_once(&self, udid: &str, state: &mut DeviceState) -> Result<IosWdaProbe, IosWdaError> {
        let ready = self.ensure_ready(udid, state)?;
        let screen_path = session_path(&ready.session_id, "/wda/screen")?;
        let screen = match self.request_json(
            ready.tunnel.endpoint,
            Method::GET,
            &screen_path,
            None,
            self.config.request_timeout,
            RequestSafety::Safe,
            MAX_JSON_RESPONSE_BYTES,
        ) {
            Ok(value) => parse_screen(&value).ok(),
            Err(error) if error.kind() == IosWdaErrorKind::InvalidSession => return Err(error),
            Err(_) => None,
        };
        let probe = IosWdaProbe {
            ready: true,
            capabilities: vec![
                IosWdaCapability::Tap,
                IosWdaCapability::Swipe,
                IosWdaCapability::TypeText,
                IosWdaCapability::KeyEvent,
                IosWdaCapability::OpenApp,
                IosWdaCapability::Screenshot,
            ],
            screen,
        };
        state.probe = Some(CachedProbe {
            at: Instant::now(),
            value: probe.clone(),
        });
        Ok(probe)
    }

    fn ensure_ready(
        &self,
        udid: &str,
        state: &mut DeviceState,
    ) -> Result<ReadyContext, IosWdaError> {
        if state
            .last_ready
            .is_some_and(|ready| ready.elapsed() <= self.config.probe_cache_ttl)
            && state.session_id.is_some()
            && self.tunnels.is_alive(udid)
        {
            let tunnel = self.tunnels.connection(udid)?;
            return Ok(ReadyContext {
                tunnel,
                session_id: state.session_id.clone().expect("session verifiee"),
            });
        }

        let tunnel = self.tunnels.connection(udid)?;
        let status = self.request_json(
            tunnel.endpoint,
            Method::GET,
            "/status",
            None,
            self.config.request_timeout,
            RequestSafety::Safe,
            MAX_JSON_RESPONSE_BYTES,
        )?;
        if !status_ready(&status) {
            state.clear_session();
            return Err(IosWdaError::new(
                IosWdaErrorKind::Unavailable,
                "WebDriverAgent n'est pas pret sur cet appareil",
            ));
        }

        if let Some(session_id) = state.session_id.clone() {
            let path = session_path(&session_id, "")?;
            match self.request_json(
                tunnel.endpoint,
                Method::GET,
                &path,
                None,
                self.config.request_timeout,
                RequestSafety::Safe,
                MAX_JSON_RESPONSE_BYTES,
            ) {
                Ok(_) => {}
                Err(error) if error.kind() == IosWdaErrorKind::InvalidSession => {
                    state.clear_session();
                }
                Err(error) => return Err(error),
            }
        }

        if state.session_id.is_none() {
            let payload = json!({
                "capabilities": {
                    "alwaysMatch": {
                        "shouldWaitForQuiescence": false,
                        "shouldUseCompactResponses": true
                    },
                    "firstMatch": [{}]
                }
            });
            let response = self.request_json(
                tunnel.endpoint,
                Method::POST,
                "/session",
                Some(&payload),
                self.config.session_timeout,
                RequestSafety::Safe,
                MAX_JSON_RESPONSE_BYTES,
            )?;
            state.session_id = Some(parse_session_id(&response)?);
        }

        state.last_ready = Some(Instant::now());
        Ok(ReadyContext {
            tunnel,
            session_id: state.session_id.clone().expect("session creee"),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn request_json(
        &self,
        endpoint: SocketAddr,
        method: Method,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
        safety: RequestSafety,
        max_response_bytes: usize,
    ) -> Result<Value, IosWdaError> {
        validate_loopback(endpoint)?;
        if !path.starts_with('/') || path.contains("//") || path.contains(['\r', '\n']) {
            return Err(IosWdaError::new(
                IosWdaErrorKind::InvalidInput,
                "chemin WDA local invalide",
            ));
        }
        let url = format!("http://{endpoint}{path}");
        let mut request = self.client.request(method, url).timeout(timeout);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().map_err(|error| {
            let kind = if safety == RequestSafety::Mutation {
                IosWdaErrorKind::AmbiguousMutation
            } else if error.is_timeout() {
                IosWdaErrorKind::Timeout
            } else {
                IosWdaErrorKind::Unavailable
            };
            let message = if kind == IosWdaErrorKind::AmbiguousMutation {
                "resultat WDA ambigu : la mutation ne sera pas rejouee"
            } else if kind == IosWdaErrorKind::Timeout {
                "delai WDA depasse"
            } else {
                "endpoint WDA local indisponible"
            };
            IosWdaError::new(kind, message)
        })?;
        let (status, bytes) = read_bounded(response, max_response_bytes).map_err(|error| {
            if safety == RequestSafety::Mutation {
                IosWdaError::new(
                    IosWdaErrorKind::AmbiguousMutation,
                    "resultat WDA ambigu : la mutation ne sera pas rejouee",
                )
            } else {
                error
            }
        })?;
        let value = serde_json::from_slice::<Value>(&bytes).map_err(|_| {
            if safety == RequestSafety::Mutation {
                IosWdaError::new(
                    IosWdaErrorKind::AmbiguousMutation,
                    "resultat WDA ambigu : la mutation ne sera pas rejouee",
                )
            } else {
                IosWdaError::new(IosWdaErrorKind::Protocol, "reponse JSON WDA invalide")
            }
        })?;
        classify_wda_response(status, value)
    }
}

#[derive(Default)]
struct DeviceState {
    session_id: Option<String>,
    last_ready: Option<Instant>,
    probe: Option<CachedProbe>,
}

impl DeviceState {
    fn clear_session(&mut self) {
        self.session_id = None;
        self.last_ready = None;
        self.probe = None;
    }
}

struct CachedProbe {
    at: Instant,
    value: IosWdaProbe,
}

struct ReadyContext {
    tunnel: Arc<IosTunnel>,
    session_id: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestSafety {
    Safe,
    Mutation,
}

enum TunnelMode {
    Iproxy(IosWdaConfig),
    Fixed(HashMap<String, SocketAddr>),
    #[cfg(test)]
    DelayedFixed(HashMap<String, SocketAddr>, Duration),
}

struct TunnelRegistry {
    mode: TunnelMode,
    tunnels: Mutex<HashMap<String, Arc<IosTunnel>>>,
    port_cursor: AtomicU64,
}

impl TunnelRegistry {
    fn new(mode: TunnelMode) -> Self {
        Self {
            mode,
            tunnels: Mutex::new(HashMap::new()),
            port_cursor: AtomicU64::new(0),
        }
    }

    fn connection(&self, udid: &str) -> Result<Arc<IosTunnel>, IosWdaError> {
        let used_ports = {
            let mut tunnels = lock(&self.tunnels, "registre des tunnels iOS")?;
            if let Some(tunnel) = tunnels.get(udid) {
                if tunnel.is_alive() {
                    return Ok(tunnel.clone());
                }
            }
            tunnels.remove(udid);
            tunnels
                .values()
                .map(|item| item.endpoint.port())
                .collect::<HashSet<_>>()
        };

        // Le lancement d'iproxy et son attente peuvent prendre plusieurs
        // secondes. Ils restent hors du verrou global afin que deux UDID
        // puissent ouvrir leurs tunnels en parallele.
        let candidate = match &self.mode {
            TunnelMode::Fixed(endpoints) => {
                let endpoint = endpoints.get(udid).copied().ok_or_else(|| {
                    IosWdaError::new(
                        IosWdaErrorKind::Unavailable,
                        "aucun endpoint WDA local associe a cet UDID",
                    )
                })?;
                Arc::new(IosTunnel::fixed(endpoint))
            }
            TunnelMode::Iproxy(config) => self.spawn_iproxy(udid, config, &used_ports)?,
            #[cfg(test)]
            TunnelMode::DelayedFixed(endpoints, delay) => {
                let endpoint = endpoints.get(udid).copied().ok_or_else(|| {
                    IosWdaError::new(
                        IosWdaErrorKind::Unavailable,
                        "aucun endpoint WDA local associe a cet UDID",
                    )
                })?;
                thread::sleep(*delay);
                Arc::new(IosTunnel::fixed(endpoint))
            }
        };

        let mut tunnels = lock(&self.tunnels, "registre des tunnels iOS")?;
        if let Some(winner) = tunnels.get(udid) {
            if winner.is_alive() {
                // Une course sur le meme UDID a deja publie son tunnel. La
                // destruction de `candidate` tue et attend l'iproxy perdant.
                return Ok(winner.clone());
            }
        }
        tunnels.remove(udid);
        tunnels.insert(udid.to_string(), candidate.clone());
        Ok(candidate)
    }

    fn is_alive(&self, udid: &str) -> bool {
        self.tunnels
            .lock()
            .ok()
            .and_then(|tunnels| tunnels.get(udid).cloned())
            .is_some_and(|tunnel| tunnel.is_alive())
    }

    fn invalidate(&self, udid: &str) {
        let removed = self
            .tunnels
            .lock()
            .ok()
            .and_then(|mut tunnels| tunnels.remove(udid));
        // Le Drop d'un tunnel gere tue puis attend iproxy. L'Arc doit donc etre
        // libere apres le mutex global, sans bloquer les autres UDID.
        drop(removed);
    }

    fn spawn_iproxy(
        &self,
        udid: &str,
        config: &IosWdaConfig,
        used_ports: &HashSet<u16>,
    ) -> Result<Arc<IosTunnel>, IosWdaError> {
        let span = u64::from(config.local_port_end - config.local_port_start) + 1;
        let mut hasher = DefaultHasher::new();
        udid.hash(&mut hasher);
        let seed = hasher
            .finish()
            .wrapping_add(self.port_cursor.fetch_add(1, Ordering::Relaxed));
        let attempts = usize::try_from(span)
            .unwrap_or(MAX_PORT_ATTEMPTS)
            .min(MAX_PORT_ATTEMPTS);

        for attempt in 0..attempts {
            let offset = (seed + attempt as u64) % span;
            let port = config.local_port_start + offset as u16;
            if used_ports.contains(&port) {
                continue;
            }
            let endpoint = SocketAddr::from(([127, 0, 0, 1], port));
            let mut command = Command::new(&config.iproxy_program);
            command
                .arg("-u")
                .arg(udid)
                .arg("-l")
                .arg("-s")
                .arg("127.0.0.1")
                .arg(format!("{port}:{}", config.remote_port))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            configure_hidden_process(&mut command);
            let Ok(mut child) = command.spawn() else {
                return Err(IosWdaError::new(
                    IosWdaErrorKind::Unavailable,
                    "impossible de lancer iproxy",
                ));
            };
            if wait_for_tunnel(&mut child, endpoint, config.tunnel_start_timeout).is_ok() {
                return Ok(Arc::new(IosTunnel::managed(endpoint, child)));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(IosWdaError::new(
            IosWdaErrorKind::Unavailable,
            "iproxy n'a pas ouvert de tunnel WDA loopback",
        ))
    }
}

struct IosTunnel {
    endpoint: SocketAddr,
    child: Mutex<Option<Child>>,
    fixed: bool,
}

impl IosTunnel {
    fn managed(endpoint: SocketAddr, child: Child) -> Self {
        Self {
            endpoint,
            child: Mutex::new(Some(child)),
            fixed: false,
        }
    }

    fn fixed(endpoint: SocketAddr) -> Self {
        Self {
            endpoint,
            child: Mutex::new(None),
            fixed: true,
        }
    }

    fn is_alive(&self) -> bool {
        let Ok(mut child) = self.child.lock() else {
            return false;
        };
        match child.as_mut() {
            Some(child) => child.try_wait().ok().flatten().is_none(),
            None => self.fixed,
        }
    }
}

impl Drop for IosTunnel {
    fn drop(&mut self) {
        let child = self
            .child
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(mut child) = child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait_for_tunnel(child: &mut Child, endpoint: SocketAddr, timeout: Duration) -> Result<(), ()> {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().map_err(|_| ())?.is_some() {
            return Err(());
        }
        if TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)).is_ok() {
            // Un autre processus local pourrait deja occuper ce port pendant
            // qu'iproxy est en train d'echouer. Verifier une seconde fois que
            // notre enfant est toujours vivant evite de memoriser ce service.
            thread::sleep(Duration::from_millis(40));
            if child.try_wait().map_err(|_| ())?.is_none() {
                return Ok(());
            }
            return Err(());
        }
        if Instant::now() >= deadline {
            return Err(());
        }
        thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(windows)]
fn configure_hidden_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_process(_command: &mut Command) {}

fn mutation_request(
    session_id: &str,
    action: &IosWdaAction,
) -> Result<(String, Value, &'static str), IosWdaError> {
    match action {
        IosWdaAction::Tap { x, y } => Ok((
            session_path(session_id, "/actions")?,
            json!({
                "actions": [{
                    "type": "pointer",
                    "id": "finger1",
                    "parameters": { "pointerType": "touch" },
                    "actions": [
                        { "type": "pointerMove", "duration": 0, "origin": "viewport", "x": x, "y": y },
                        { "type": "pointerDown", "button": 0 },
                        { "type": "pause", "duration": 50 },
                        { "type": "pointerUp", "button": 0 }
                    ]
                }]
            }),
            "tap WDA termine",
        )),
        IosWdaAction::Swipe {
            start_x,
            start_y,
            end_x,
            end_y,
            duration_ms,
        } => Ok((
            session_path(session_id, "/actions")?,
            json!({
                "actions": [{
                    "type": "pointer",
                    "id": "finger1",
                    "parameters": { "pointerType": "touch" },
                    "actions": [
                        { "type": "pointerMove", "duration": 0, "origin": "viewport", "x": start_x, "y": start_y },
                        { "type": "pointerDown", "button": 0 },
                        { "type": "pointerMove", "duration": duration_ms, "origin": "viewport", "x": end_x, "y": end_y },
                        { "type": "pointerUp", "button": 0 }
                    ]
                }]
            }),
            "swipe WDA termine",
        )),
        IosWdaAction::TypeText { text } => Ok((
            session_path(session_id, "/wda/keys")?,
            json!({ "value": [text] }),
            "texte WDA saisi",
        )),
        IosWdaAction::KeyEvent { key } => Ok((
            session_path(session_id, "/wda/pressButton")?,
            json!({ "name": map_button(key)? }),
            "bouton WDA presse",
        )),
        IosWdaAction::OpenApp { bundle_id } => Ok((
            session_path(session_id, "/wda/apps/launch")?,
            json!({
                "bundleId": bundle_id,
                "shouldWaitForQuiescence": false
            }),
            "application WDA ouverte",
        )),
        IosWdaAction::Screenshot => Err(IosWdaError::new(
            IosWdaErrorKind::InvalidInput,
            "la capture WDA utilise une requete sure dediee",
        )),
    }
}

fn validate_action(action: &IosWdaAction) -> Result<(), IosWdaError> {
    match action {
        IosWdaAction::Tap { x, y } => validate_coordinates(&[*x, *y]),
        IosWdaAction::Swipe {
            start_x,
            start_y,
            end_x,
            end_y,
            duration_ms,
        } => {
            validate_coordinates(&[*start_x, *start_y, *end_x, *end_y])?;
            if !(1..=MAX_GESTURE_DURATION_MS).contains(duration_ms) {
                return Err(IosWdaError::new(
                    IosWdaErrorKind::InvalidInput,
                    "duree de swipe WDA invalide",
                ));
            }
            Ok(())
        }
        IosWdaAction::TypeText { text } => {
            if text.is_empty() || text.len() > MAX_TEXT_BYTES || text.contains('\0') {
                return Err(IosWdaError::new(
                    IosWdaErrorKind::InvalidInput,
                    "texte WDA vide ou trop long",
                ));
            }
            Ok(())
        }
        IosWdaAction::KeyEvent { key } => map_button(key).map(|_| ()),
        IosWdaAction::OpenApp { bundle_id } => validate_bundle_id(bundle_id),
        IosWdaAction::Screenshot => Ok(()),
    }
}

fn validate_coordinates(values: &[u32]) -> Result<(), IosWdaError> {
    if values.iter().any(|value| *value > MAX_COORDINATE) {
        return Err(IosWdaError::new(
            IosWdaErrorKind::InvalidInput,
            "coordonnees WDA hors limites",
        ));
    }
    Ok(())
}

fn map_button(value: &str) -> Result<&'static str, IosWdaError> {
    match value.trim().to_ascii_uppercase().as_str() {
        "HOME" => Ok("home"),
        "VOLUME_UP" => Ok("volumeup"),
        "VOLUME_DOWN" => Ok("volumedown"),
        _ => Err(IosWdaError::new(
            IosWdaErrorKind::InvalidInput,
            "bouton WDA non supporte (HOME, VOLUME_UP ou VOLUME_DOWN uniquement)",
        )),
    }
}

fn validate_bundle_id(bundle_id: &str) -> Result<(), IosWdaError> {
    if bundle_id.is_empty()
        || bundle_id.len() > MAX_BUNDLE_ID_BYTES
        || !bundle_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        || !bundle_id.contains('.')
    {
        return Err(IosWdaError::new(
            IosWdaErrorKind::InvalidInput,
            "bundle ID iOS invalide",
        ));
    }
    Ok(())
}

fn validate_udid(udid: &str) -> Result<(), IosWdaError> {
    if udid.is_empty()
        || udid.len() > MAX_UDID_BYTES
        || !udid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(IosWdaError::new(
            IosWdaErrorKind::InvalidInput,
            "UDID iOS invalide",
        ));
    }
    Ok(())
}

fn session_path(session_id: &str, suffix: &str) -> Result<String, IosWdaError> {
    if session_id.is_empty()
        || session_id.len() > MAX_SESSION_ID_BYTES
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || (!suffix.is_empty() && !suffix.starts_with('/'))
    {
        return Err(IosWdaError::new(
            IosWdaErrorKind::Protocol,
            "identifiant de session WDA invalide",
        ));
    }
    Ok(format!("/session/{session_id}{suffix}"))
}

fn status_ready(value: &Value) -> bool {
    value
        .pointer("/value/ready")
        .and_then(Value::as_bool)
        .or_else(|| value.get("ready").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn parse_session_id(value: &Value) -> Result<String, IosWdaError> {
    let session_id = value
        .pointer("/value/sessionId")
        .and_then(Value::as_str)
        .or_else(|| value.get("sessionId").and_then(Value::as_str))
        .ok_or_else(|| {
            IosWdaError::new(
                IosWdaErrorKind::Protocol,
                "WDA n'a pas retourne de sessionId",
            )
        })?;
    session_path(session_id, "")?;
    Ok(session_id.to_string())
}

fn parse_screen(value: &Value) -> Result<IosWdaScreen, IosWdaError> {
    let root = value.get("value").unwrap_or(value);
    let screen = root.get("screenSize").unwrap_or(root);
    let width = screen
        .get("width")
        .and_then(Value::as_f64)
        .ok_or_else(|| IosWdaError::new(IosWdaErrorKind::Protocol, "largeur ecran WDA absente"))?;
    let height = screen
        .get("height")
        .and_then(Value::as_f64)
        .ok_or_else(|| IosWdaError::new(IosWdaErrorKind::Protocol, "hauteur ecran WDA absente"))?;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(IosWdaError::new(
            IosWdaErrorKind::Protocol,
            "dimensions ecran WDA invalides",
        ));
    }
    let scale = root
        .get("scale")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0);
    let status_bar_height = root
        .pointer("/statusBarSize/height")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0);
    Ok(IosWdaScreen {
        width,
        height,
        scale,
        status_bar_height,
    })
}

fn decode_screenshot(value: &Value) -> Result<IosWdaScreenshot, IosWdaError> {
    let encoded = value
        .get("value")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .ok_or_else(|| {
            IosWdaError::new(IosWdaErrorKind::Protocol, "capture WDA sans donnees base64")
        })?;
    let max_encoded = ((MAX_SCREENSHOT_BYTES + 2) / 3) * 4;
    if encoded.len() > max_encoded {
        return Err(IosWdaError::new(
            IosWdaErrorKind::ResponseTooLarge,
            "capture WDA superieure a 8 Mio",
        ));
    }
    let bytes = STANDARD.decode(encoded).map_err(|_| {
        IosWdaError::new(IosWdaErrorKind::Protocol, "capture WDA en base64 invalide")
    })?;
    if bytes.len() > MAX_SCREENSHOT_BYTES {
        return Err(IosWdaError::new(
            IosWdaErrorKind::ResponseTooLarge,
            "capture WDA superieure a 8 Mio",
        ));
    }
    let mime_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        "image/tiff"
    } else {
        return Err(IosWdaError::new(
            IosWdaErrorKind::Protocol,
            "signature de capture WDA invalide",
        ));
    };
    Ok(IosWdaScreenshot {
        bytes,
        mime_type: mime_type.to_string(),
    })
}

fn classify_wda_response(status: StatusCode, value: Value) -> Result<Value, IosWdaError> {
    let error_code = value
        .pointer("/value/error")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str));
    if error_code.is_some_and(|code| {
        code.eq_ignore_ascii_case("invalid session id")
            || code.eq_ignore_ascii_case("invalid session")
    }) {
        return Err(IosWdaError::new(
            IosWdaErrorKind::InvalidSession,
            "session WDA invalide",
        ));
    }
    if !status.is_success() || error_code.is_some() {
        let safe_code = error_code
            .filter(|code| {
                code.len() <= 80
                    && code.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'_')
                    })
            })
            .unwrap_or("erreur WebDriverAgent");
        return Err(IosWdaError::new(
            IosWdaErrorKind::Protocol,
            format!("WDA a refuse la requete ({safe_code})"),
        ));
    }
    Ok(value)
}

fn read_bounded(
    response: Response,
    max_response_bytes: usize,
) -> Result<(StatusCode, Vec<u8>), IosWdaError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes as u64)
    {
        return Err(IosWdaError::new(
            IosWdaErrorKind::ResponseTooLarge,
            "reponse WDA trop volumineuse",
        ));
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(0)
            .min(max_response_bytes as u64) as usize,
    );
    response
        .take(max_response_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            IosWdaError::new(
                IosWdaErrorKind::Unavailable,
                "lecture de la reponse WDA interrompue",
            )
        })?;
    if bytes.len() > max_response_bytes {
        return Err(IosWdaError::new(
            IosWdaErrorKind::ResponseTooLarge,
            "reponse WDA trop volumineuse",
        ));
    }
    Ok((status, bytes))
}

fn validate_loopback(endpoint: SocketAddr) -> Result<(), IosWdaError> {
    if endpoint.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || endpoint.port() == 0 {
        return Err(IosWdaError::new(
            IosWdaErrorKind::InvalidInput,
            "l'endpoint WDA doit etre une adresse loopback avec un port non nul",
        ));
    }
    Ok(())
}

fn validate_config(config: &IosWdaConfig) -> Result<(), IosWdaError> {
    if config.iproxy_program.as_os_str().is_empty()
        || config.remote_port == 0
        || config.local_port_start < 1024
        || config.local_port_start > config.local_port_end
        || u32::from(config.local_port_end) - u32::from(config.local_port_start) > 10_000
    {
        return Err(IosWdaError::new(
            IosWdaErrorKind::InvalidInput,
            "configuration de ports WDA invalide",
        ));
    }
    validate_duration(
        config.probe_cache_ttl,
        Duration::from_secs(5),
        Duration::from_secs(10),
        "cache probe WDA",
    )?;
    validate_duration(
        config.connect_timeout,
        Duration::from_millis(100),
        Duration::from_secs(5),
        "connexion WDA",
    )?;
    validate_duration(
        config.request_timeout,
        Duration::from_secs(1),
        Duration::from_secs(30),
        "requete WDA",
    )?;
    validate_duration(
        config.session_timeout,
        Duration::from_secs(1),
        Duration::from_secs(60),
        "session WDA",
    )?;
    validate_duration(
        config.screenshot_timeout,
        Duration::from_secs(1),
        Duration::from_secs(30),
        "capture WDA",
    )?;
    validate_duration(
        config.tunnel_start_timeout,
        Duration::from_millis(500),
        Duration::from_secs(10),
        "demarrage iproxy",
    )
}

fn validate_duration(
    value: Duration,
    minimum: Duration,
    maximum: Duration,
    label: &str,
) -> Result<(), IosWdaError> {
    if value < minimum || value > maximum {
        return Err(IosWdaError::new(
            IosWdaErrorKind::InvalidInput,
            format!("delai {label} hors limites"),
        ));
    }
    Ok(())
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u16(name: &str, default: u16) -> Result<u16, IosWdaError> {
    match non_empty_env(name) {
        Some(value) => value.parse::<u16>().map_err(|_| {
            IosWdaError::new(
                IosWdaErrorKind::InvalidInput,
                format!("variable {name} invalide"),
            )
        }),
        None => Ok(default),
    }
}

fn env_u64(name: &str, default: u64) -> Result<u64, IosWdaError> {
    match non_empty_env(name) {
        Some(value) => value.parse::<u64>().map_err(|_| {
            IosWdaError::new(
                IosWdaErrorKind::InvalidInput,
                format!("variable {name} invalide"),
            )
        }),
        None => Ok(default),
    }
}

fn lock<'a, T>(mutex: &'a Mutex<T>, label: &str) -> Result<MutexGuard<'a, T>, IosWdaError> {
    mutex
        .lock()
        .map_err(|_| IosWdaError::new(IosWdaErrorKind::Unavailable, format!("{label} verrouille")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread::JoinHandle;

    #[derive(Clone, Debug)]
    struct RecordedRequest {
        method: String,
        path: String,
        body: Vec<u8>,
    }

    enum FakeReply {
        Json(u16, Value),
        Raw {
            status: u16,
            body: Vec<u8>,
            declared_length: usize,
        },
        Disconnect,
    }

    struct FakeServer {
        endpoint: SocketAddr,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        handle: JoinHandle<()>,
    }

    impl FakeServer {
        fn start(replies: Vec<FakeReply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("fake WDA listener");
            let endpoint = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let thread_requests = requests.clone();
            let handle = std::thread::spawn(move || {
                let mut replies = VecDeque::from(replies);
                while let Some(reply) = replies.pop_front() {
                    let (mut stream, _) = listener.accept().expect("fake WDA accept");
                    let request = read_fake_request(&mut stream);
                    thread_requests.lock().unwrap().push(request);
                    match reply {
                        FakeReply::Json(status, value) => {
                            write_fake_json(&mut stream, status, &value)
                        }
                        FakeReply::Raw {
                            status,
                            body,
                            declared_length,
                        } => write_fake_raw(&mut stream, status, &body, declared_length),
                        FakeReply::Disconnect => {}
                    }
                }
            });
            Self {
                endpoint,
                requests,
                handle,
            }
        }

        fn finish(self) -> Vec<RecordedRequest> {
            self.handle.join().expect("fake WDA thread");
            Arc::try_unwrap(self.requests)
                .expect("fake request references")
                .into_inner()
                .unwrap()
        }
    }

    fn read_fake_request(stream: &mut TcpStream) -> RecordedRequest {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let count = stream.read(&mut chunk).expect("fake request read");
            assert!(count > 0, "request closed before headers");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break index + 4;
            }
            assert!(bytes.len() <= 64 * 1024, "request headers too large");
        };
        let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let content_length = header
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
            })
            .unwrap_or(0);
        while bytes.len() - header_end < content_length {
            let count = stream.read(&mut chunk).expect("fake request body");
            assert!(count > 0, "request closed before body");
            bytes.extend_from_slice(&chunk[..count]);
        }
        let request_line = header.lines().next().unwrap();
        let mut parts = request_line.split_whitespace();
        RecordedRequest {
            method: parts.next().unwrap().to_string(),
            path: parts.next().unwrap().to_string(),
            body: bytes[header_end..header_end + content_length].to_vec(),
        }
    }

    fn write_fake_json(stream: &mut TcpStream, status: u16, value: &Value) {
        let body = serde_json::to_vec(value).unwrap();
        write_fake_raw(stream, status, &body, body.len());
    }

    fn write_fake_raw(stream: &mut TcpStream, status: u16, body: &[u8], declared_length: usize) {
        let reason = if status >= 400 { "Error" } else { "OK" };
        let _ = write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            declared_length
        );
        let _ = stream.write_all(body);
        let _ = stream.flush();
    }

    fn provider(udid: &str, endpoint: SocketAddr) -> IosWdaProvider {
        IosWdaProvider::with_loopback_endpoint_for_tests(udid, endpoint).unwrap()
    }

    fn ready() -> FakeReply {
        FakeReply::Json(200, json!({ "value": { "ready": true } }))
    }

    fn session(id: &str) -> FakeReply {
        FakeReply::Json(200, json!({ "value": { "sessionId": id } }))
    }

    fn ok() -> FakeReply {
        FakeReply::Json(200, json!({ "value": null }))
    }

    #[test]
    fn probe_requires_ready_session_and_reads_screen_metrics() {
        let server = FakeServer::start(vec![
            ready(),
            session("session-a"),
            FakeReply::Json(
                200,
                json!({
                    "value": {
                        "screenSize": { "width": 390, "height": 844 },
                        "statusBarSize": { "width": 390, "height": 47 },
                        "scale": 3
                    }
                }),
            ),
        ]);
        let provider = provider("udid-a", server.endpoint);
        let probe = provider.probe("udid-a").unwrap();
        let cached_probe = provider.probe("udid-a").unwrap();
        assert!(probe.ready);
        assert_eq!(cached_probe, probe);
        assert_eq!(probe.capabilities.len(), 6);
        assert_eq!(
            probe.screen,
            Some(IosWdaScreen {
                width: 390.0,
                height: 844.0,
                scale: Some(3.0),
                status_bar_height: Some(47.0),
            })
        );
        let requests = server.finish();
        assert_eq!(
            requests
                .iter()
                .map(|request| (request.method.as_str(), request.path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("GET", "/status"),
                ("POST", "/session"),
                ("GET", "/session/session-a/wda/screen")
            ]
        );
        let session_body: Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(
            session_body,
            json!({
                "capabilities": {
                    "alwaysMatch": {
                        "shouldWaitForQuiescence": false,
                        "shouldUseCompactResponses": true
                    },
                    "firstMatch": [{}]
                }
            })
        );
    }

    #[test]
    fn parses_w3c_and_legacy_session_ids() {
        assert_eq!(
            parse_session_id(&json!({ "value": { "sessionId": "w3c-1" } })).unwrap(),
            "w3c-1"
        );
        assert_eq!(
            parse_session_id(&json!({ "sessionId": "legacy-1", "value": {} })).unwrap(),
            "legacy-1"
        );
    }

    #[test]
    fn emits_exact_payloads_for_supported_mutations() {
        let server = FakeServer::start(vec![
            ready(),
            session("session-actions"),
            ok(),
            ok(),
            ok(),
            ok(),
            ok(),
        ]);
        let provider = provider("udid-actions", server.endpoint);
        provider
            .execute("udid-actions", IosWdaAction::Tap { x: 12, y: 34 })
            .unwrap();
        provider
            .execute(
                "udid-actions",
                IosWdaAction::Swipe {
                    start_x: 1,
                    start_y: 2,
                    end_x: 300,
                    end_y: 400,
                    duration_ms: 750,
                },
            )
            .unwrap();
        provider
            .execute(
                "udid-actions",
                IosWdaAction::TypeText {
                    text: "bonjour".to_string(),
                },
            )
            .unwrap();
        provider
            .execute(
                "udid-actions",
                IosWdaAction::KeyEvent {
                    key: "VOLUME_UP".to_string(),
                },
            )
            .unwrap();
        provider
            .execute(
                "udid-actions",
                IosWdaAction::OpenApp {
                    bundle_id: "com.example.app".to_string(),
                },
            )
            .unwrap();

        let requests = server.finish();
        let actions = &requests[2..];
        assert_eq!(actions[0].path, "/session/session-actions/actions");
        assert_eq!(actions[1].path, "/session/session-actions/actions");
        assert_eq!(actions[2].path, "/session/session-actions/wda/keys");
        assert_eq!(actions[3].path, "/session/session-actions/wda/pressButton");
        assert_eq!(actions[4].path, "/session/session-actions/wda/apps/launch");

        let tap: Value = serde_json::from_slice(&actions[0].body).unwrap();
        assert_eq!(tap["actions"][0]["parameters"]["pointerType"], "touch");
        assert_eq!(tap["actions"][0]["actions"][0]["origin"], "viewport");
        assert_eq!(tap["actions"][0]["actions"][0]["x"], 12);
        assert_eq!(tap["actions"][0]["actions"][0]["y"], 34);
        let swipe: Value = serde_json::from_slice(&actions[1].body).unwrap();
        assert_eq!(swipe["actions"][0]["actions"][2]["duration"], 750);
        assert_eq!(swipe["actions"][0]["actions"][2]["x"], 300);
        assert_eq!(swipe["actions"][0]["actions"][2]["y"], 400);
        assert_eq!(
            serde_json::from_slice::<Value>(&actions[2].body).unwrap(),
            json!({ "value": ["bonjour"] })
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&actions[3].body).unwrap(),
            json!({ "name": "volumeup" })
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&actions[4].body).unwrap(),
            json!({
                "bundleId": "com.example.app",
                "shouldWaitForQuiescence": false
            })
        );
    }

    #[test]
    fn refuses_unadvertised_buttons_before_network_io() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let provider = provider("udid-key", listener.local_addr().unwrap());
        let error = provider
            .execute(
                "udid-key",
                IosWdaAction::KeyEvent {
                    key: "ENTER".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(error.kind(), IosWdaErrorKind::InvalidInput);
    }

    #[test]
    fn screenshot_uses_standalone_route_and_validates_signature() {
        let server = FakeServer::start(vec![
            ready(),
            session("session-shot"),
            FakeReply::Json(200, json!({ "value": STANDARD.encode(b"not an image") })),
        ]);
        let provider = provider("udid-shot", server.endpoint);
        let error = provider
            .execute("udid-shot", IosWdaAction::Screenshot)
            .unwrap_err();
        assert_eq!(error.kind(), IosWdaErrorKind::Protocol);
        let requests = server.finish();
        assert_eq!(requests[2].method, "GET");
        assert_eq!(requests[2].path, "/screenshot");
    }

    #[test]
    fn screenshot_rejects_more_than_eight_mebibytes() {
        let oversized = vec![0u8; MAX_SCREENSHOT_BYTES + 1];
        let error = decode_screenshot(&json!({ "value": STANDARD.encode(oversized) })).unwrap_err();
        assert_eq!(error.kind(), IosWdaErrorKind::ResponseTooLarge);
    }

    #[test]
    fn mutation_disconnect_is_ambiguous_and_never_replayed() {
        let server = FakeServer::start(vec![
            ready(),
            session("session-once"),
            FakeReply::Disconnect,
        ]);
        let provider = provider("udid-once", server.endpoint);
        let error = provider
            .execute("udid-once", IosWdaAction::Tap { x: 9, y: 8 })
            .unwrap_err();
        assert_eq!(error.kind(), IosWdaErrorKind::AmbiguousMutation);
        let requests = server.finish();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.ends_with("/actions"))
                .count(),
            1
        );
    }

    #[test]
    fn malformed_truncated_or_oversized_mutation_response_is_ambiguous() {
        let cases = [
            FakeReply::Raw {
                status: 200,
                body: b"{not-json".to_vec(),
                declared_length: b"{not-json".len(),
            },
            FakeReply::Raw {
                status: 200,
                body: b"{}".to_vec(),
                declared_length: MAX_JSON_RESPONSE_BYTES + 1,
            },
            FakeReply::Raw {
                status: 200,
                body: b"{\"value\":null}".to_vec(),
                declared_length: b"{\"value\":null}".len() + 20,
            },
        ];

        for (index, reply) in cases.into_iter().enumerate() {
            let server = FakeServer::start(vec![ready(), session("session-bad-body"), reply]);
            let udid = format!("udid-bad-body-{index}");
            let provider = provider(&udid, server.endpoint);
            let error = provider
                .execute(&udid, IosWdaAction::Tap { x: 2, y: 3 })
                .unwrap_err();
            assert_eq!(error.kind(), IosWdaErrorKind::AmbiguousMutation);
            let requests = server.finish();
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| request.path.ends_with("/actions"))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn slow_tunnel_creation_for_two_udids_is_not_globally_serialized() {
        let listener_a = TcpListener::bind("127.0.0.1:0").unwrap();
        let listener_b = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoints = HashMap::from([
            ("udid-slow-a".to_string(), listener_a.local_addr().unwrap()),
            ("udid-slow-b".to_string(), listener_b.local_addr().unwrap()),
        ]);
        let registry = Arc::new(TunnelRegistry::new(TunnelMode::DelayedFixed(
            endpoints,
            Duration::from_millis(500),
        )));
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for udid in ["udid-slow-a", "udid-slow-b"] {
            let registry = registry.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                registry.connection(udid).unwrap()
            }));
        }
        let started = Instant::now();
        barrier.wait();
        let tunnels = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert!(
            started.elapsed() < Duration::from_millis(850),
            "les creations de tunnel semblent serialisees"
        );
        assert_ne!(tunnels[0].endpoint, tunnels[1].endpoint);
    }

    #[test]
    fn two_udids_are_isolated_on_distinct_loopback_endpoints() {
        let server_a = FakeServer::start(vec![ready(), session("session-a"), ok()]);
        let server_b = FakeServer::start(vec![ready(), session("session-b"), ok()]);
        let provider = IosWdaProvider::with_loopback_endpoints_for_tests(HashMap::from([
            ("udid-a".to_string(), server_a.endpoint),
            ("udid-b".to_string(), server_b.endpoint),
        ]))
        .unwrap();
        provider
            .execute(
                "udid-a",
                IosWdaAction::OpenApp {
                    bundle_id: "com.example.alpha".to_string(),
                },
            )
            .unwrap();
        provider
            .execute(
                "udid-b",
                IosWdaAction::OpenApp {
                    bundle_id: "com.example.beta".to_string(),
                },
            )
            .unwrap();

        let requests_a = server_a.finish();
        let requests_b = server_b.finish();
        assert!(String::from_utf8_lossy(&requests_a[2].body).contains("com.example.alpha"));
        assert!(!String::from_utf8_lossy(&requests_a[2].body).contains("com.example.beta"));
        assert!(String::from_utf8_lossy(&requests_b[2].body).contains("com.example.beta"));
    }

    #[test]
    fn non_loopback_test_endpoint_is_rejected() {
        let error = IosWdaProvider::with_loopback_endpoint_for_tests(
            "udid-remote",
            SocketAddr::new(IpAddr::from([192, 0, 2, 1]), 8100),
        )
        .err()
        .expect("non-loopback endpoint must fail");
        assert_eq!(error.kind(), IosWdaErrorKind::InvalidInput);
    }
}
