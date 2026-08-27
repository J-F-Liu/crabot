//! Mirrors the Windows system proxy (HKCU `Internet Settings`) into
//! `http_proxy`/`https_proxy`/`no_proxy` once at startup, so reqwest, child
//! processes, and sandbox curl/wget route through it. Clash-style tools set
//! only the registry, which HTTP clients never read; other platforms rely on
//! env vars.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// The HKCU `Internet Settings` key holding the system proxy.
#[cfg(windows)]
const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// Proxy env vars (lowercase) paired with the `SystemProxy` field index.
/// Windows env lookups are case-insensitive, so uppercase-only readers see
/// them too; Unix tools conventionally read lowercase.
const PROXY_ENVS: [(&str, usize); 4] = [
    ("http_proxy", 0),
    ("https_proxy", 1),
    ("all_proxy", 0),
    ("no_proxy", 2),
];

/// Both cases checked when detecting an explicit user proxy. Windows env
/// lookups are case-insensitive, but Unix ones are not: a user-set uppercase
/// `HTTP_PROXY` must still be detected there, or the proxy gates would go
/// disabled and apply `no_proxy()` to the clients, bypassing the user's proxy.
const EXPLICIT_PROXY_ENVS: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// Parsed system proxy.
#[derive(Debug)]
struct SystemProxy {
    http: String,
    https: String,
    no_proxy: String,
}

impl SystemProxy {
    /// Env vars mirroring this proxy (lowercase names).
    fn env_pairs(&self) -> [(&'static str, &str); 4] {
        let fields = [
            self.http.as_str(),
            self.https.as_str(),
            self.no_proxy.as_str(),
        ];
        std::array::from_fn(|i| (PROXY_ENVS[i].0, fields[PROXY_ENVS[i].1]))
    }
}

/// Startup proxy gates; unset = enabled.
static CONFIG: OnceLock<ProxyConfig> = OnceLock::new();

/// True once `configure_proxy` has run — lets accessors keep their
/// default-enabled fallback when it never did (e.g. in tests).
static STARTED: AtomicBool = AtomicBool::new(false);
/// Set when the startup probe lands; accessors wait on it via [`COND`].
static DONE: Mutex<bool> = Mutex::new(false);
static COND: Condvar = Condvar::new();

struct ProxyConfig {
    llm: bool,
    tools: bool,
}

/// Apply the `use_system_proxy_for_*` settings once at startup. Explicit
/// proxy env vars win; an unreachable system proxy is skipped so direct
/// connectivity keeps working. The registry probe runs on a background
/// thread — a dead proxy's 300 ms connect timeouts would otherwise stall
/// startup — and accessors block until it lands.
pub fn configure_proxy(for_llm: bool, for_tools: bool) {
    STARTED.store(true, Ordering::Release);
    if !for_llm && !for_tools {
        finish(ProxyConfig {
            llm: false,
            tools: false,
        });
        return;
    }
    // Explicit env needs no probing; adopt it on this thread.
    if explicit_proxy_env() {
        if for_tools {
            install_sandbox_transport();
        }
        finish(ProxyConfig {
            llm: for_llm,
            tools: for_tools,
        });
        return;
    }
    // Registry proxy: probe reachability on a background thread so startup
    // is never blocked on it.
    let spawned = std::thread::Builder::new()
        .name("proxy-probe".into())
        .spawn(move || probe_and_finish(for_llm, for_tools));
    if spawned.is_err() {
        // Pathological: probe on this thread so accessors never block forever.
        probe_and_finish(for_llm, for_tools);
    }
}

/// True when LLM traffic may use the system proxy (registry + env).
pub fn llm_proxy_enabled() -> bool {
    wait_probe();
    CONFIG.get().is_none_or(|c| c.llm)
}

/// True when tool HTTP is proxied; false means tool clients must `no_proxy()`
/// so reqwest's registry fallback can't proxy them anyway.
pub fn tools_proxy_active() -> bool {
    wait_probe();
    CONFIG.get().is_none_or(|c| c.tools)
}

/// Probe the registry proxy and, when it's alive, adopt it (env export and
/// sandbox transport). True when proxying is active.
fn probe_and_finish(for_llm: bool, for_tools: bool) {
    let reachable = adopt_system_proxy(for_tools);
    finish(ProxyConfig {
        llm: for_llm && reachable,
        tools: for_tools && reachable,
    });
}

fn adopt_system_proxy(for_tools: bool) -> bool {
    let Some(proxy) = system_proxy() else {
        return false;
    };
    if !proxy_reachable(&proxy) {
        tracing::debug!(http = %redact_proxy_url(&proxy.http), "system proxy unreachable; ignored");
        return false;
    }
    if for_tools {
        export_proxy_env(&proxy);
        install_sandbox_transport();
    }
    true
}

fn finish(config: ProxyConfig) {
    let _ = CONFIG.set(config);
    let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
    *done = true;
    COND.notify_all();
}

/// Block until the startup probe lands. No-op when `configure_proxy` never
/// ran — accessors then fall back to their default-enabled behavior. Clients
/// are built lazily (first LLM call, first fetch), long after the probe.
fn wait_probe() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
    while !*done {
        done = COND.wait(done).unwrap_or_else(|e| e.into_inner());
    }
}

/// Whether any explicit proxy env var is set (either case).
fn explicit_proxy_env() -> bool {
    EXPLICIT_PROXY_ENVS
        .iter()
        .any(|k| std::env::var_os(k).is_some())
}

/// Whether the user set `NO_PROXY`/`no_proxy` explicitly.
fn explicit_no_proxy_env() -> bool {
    ["NO_PROXY", "no_proxy"]
        .iter()
        .any(|k| std::env::var_os(k).is_some())
}

/// Proxy URL with credentials masked for logging.
fn redact_proxy_url(raw: &str) -> String {
    if let Ok(mut url) = reqwest::Url::parse(raw)
        && url.host_str().is_some()
    {
        if !url.username().is_empty() || url.password().is_some() {
            let _ = url.set_username("***");
            let _ = url.set_password(Some("***"));
            return url.to_string();
        }
        return raw.to_string();
    }
    // Scheme-less `user:pass@host:port` parses as an opaque URL; mask it.
    match raw.rsplit_once('@') {
        Some((userinfo, rest)) if !userinfo.is_empty() => format!("***@{rest}"),
        _ => raw.to_string(),
    }
}

/// Export the proxy as lowercase `http_proxy`/`https_proxy`/`no_proxy` env
/// vars (case-insensitive on Windows, conventional on Unix). A user-set
/// `NO_PROXY` wins over the registry's override list — writing our derived
/// value would silently replace it (Windows env is case-insensitive).
fn export_proxy_env(proxy: &SystemProxy) {
    // SAFETY: only the (single) probe thread mutates env, once, before any
    // HTTP client is built; concurrent readers don't touch env at startup.
    let keep_user_no_proxy = explicit_no_proxy_env();
    for (key, value) in proxy.env_pairs() {
        if keep_user_no_proxy && key == "no_proxy" {
            continue;
        }
        unsafe { std::env::set_var(key, value) };
    }
    if keep_user_no_proxy {
        tracing::info!(
            http = %redact_proxy_url(&proxy.http),
            https = %redact_proxy_url(&proxy.https),
            "applied system proxy (keeping user NO_PROXY)",
        );
    } else {
        tracing::info!(
            http = %redact_proxy_url(&proxy.http),
            https = %redact_proxy_url(&proxy.https),
            no_proxy = %proxy.no_proxy,
            "applied system proxy",
        );
    }
}

/// Install the sandbox curl/wget transport (env-driven; on failure curl
/// stays on bashkit's direct transport).
fn install_sandbox_transport() {
    match SystemProxyTransport::new() {
        Ok(transport) => {
            let _ = EGRESS.set(Arc::new(transport));
        }
        Err(error) => {
            tracing::warn!(%error, "proxy transport build failed; sandbox curl stays direct")
        }
    }
}

// ── Registry (Windows) ─────────────────────────────────────────────

/// Read the system proxy from HKCU `Internet Settings`.
#[cfg(windows)]
fn system_proxy() -> Option<SystemProxy> {
    let key = windows_registry::CURRENT_USER
        .open(INTERNET_SETTINGS)
        .ok()?;
    if key.get_u32("ProxyEnable").ok()? == 0 {
        return None;
    }
    let server = key.get_string("ProxyServer").ok();
    let override_ = key.get_string("ProxyOverride").ok();
    let (http, https) = parse_proxy_server(server.as_deref()?)?;
    Some(SystemProxy {
        http,
        https,
        no_proxy: parse_no_proxy(override_.as_deref()),
    })
}

/// No registry system proxy on non-Windows hosts.
#[cfg(not(windows))]
fn system_proxy() -> Option<SystemProxy> {
    None
}

/// Parse `ProxyServer` into `(http, https)` URLs. A bare `host:port` serves
/// both schemes; `http=...;https=...` entries map individually (others are
/// skipped); a missing entry falls back to the other scheme's proxy. Values
/// without `://` are plain-HTTP proxies — Windows uses the key to select the
/// proxied scheme, not the proxy's protocol.
#[cfg(windows)]
fn parse_proxy_server(server: &str) -> Option<(String, String)> {
    let server = server.trim();
    if server.is_empty() {
        return None;
    }
    if !server.contains('=') {
        let url = proxy_url("http", server);
        return Some((url.clone(), url));
    }
    let mut http = None;
    let mut https = None;
    for (scheme, addr) in server.split(';').filter_map(|p| p.split_once('=')) {
        let addr = addr.trim();
        if addr.is_empty() {
            continue;
        }
        // Bare addresses are plain-HTTP proxies; only an explicit `://` URL
        // picks another proxy protocol.
        let url = proxy_url("http", addr);
        match scheme.trim().to_ascii_lowercase().as_str() {
            "http" => http = Some(url.clone()),
            "https" => https = Some(url),
            _ => {}
        }
    }
    let http = http.or_else(|| https.clone())?;
    Some((http.clone(), https.unwrap_or(http)))
}

/// `host:port` → `{scheme}://host:port`; URLs pass through unchanged.
#[cfg(windows)]
fn proxy_url(scheme: &str, addr: &str) -> String {
    if addr.contains("://") {
        addr.to_string()
    } else {
        format!("{scheme}://{addr}")
    }
}

/// NO_PROXY list: loopback always, plus `ProxyOverride` entries mapped onto
/// hyper-util's NO_PROXY syntax.
#[cfg(windows)]
fn parse_no_proxy(override_: Option<&str>) -> String {
    let mut entries = vec!["localhost", "127.0.0.1", "::1"];
    if let Some(list) = override_ {
        entries.extend(list.split(';').filter_map(normalize_no_proxy_entry));
    }
    entries.join(",")
}

/// One `ProxyOverride` entry → hyper-util NO_PROXY syntax. `*.example.com`
/// becomes `example.com`; `<local>` and `127.*`-style wildcards the matcher
/// cannot express are dropped rather than silently proxied.
#[cfg(windows)]
fn normalize_no_proxy_entry(entry: &str) -> Option<&str> {
    let entry = entry.trim();
    let entry = entry.strip_prefix("*.").unwrap_or(entry);
    if entry.is_empty()
        || entry.contains('*')
        || entry.eq_ignore_ascii_case("<local>")
        || entry.eq_ignore_ascii_case("localhost")
    {
        return None;
    }
    Some(entry)
}

// ── Reachability ───────────────────────────────────────────────────

/// True when the proxy accepts connections — guards stale registry entries
/// left by a crashed proxy app.
fn proxy_reachable(proxy: &SystemProxy) -> bool {
    let Some((host, port)) = proxy_host_port(&proxy.http) else {
        return false;
    };
    let Ok(addrs) = (host.as_str(), port).to_socket_addrs() else {
        return false;
    };
    addrs
        .into_iter()
        .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok())
}

/// `http(s)://host[:port]` → `(host, port)`; missing ports use the scheme's
/// default.
fn proxy_host_port(url: &str) -> Option<(String, u16)> {
    let url = reqwest::Url::parse(url).ok()?;
    Some((url.host_str()?.to_string(), url.port_or_known_default()?))
}

// ── Sandbox transport ──────────────────────────────────────────────

/// reqwest transport routing sandbox curl/wget through the proxy; bashkit's
/// allowlist/SSRF policy still runs first.
struct SystemProxyTransport {
    client: reqwest::Client,
}

impl SystemProxyTransport {
    /// Env-driven client — reads the vars `adopt_system_proxy` exported.
    fn new() -> Result<Self, String> {
        Ok(Self {
            client: Self::base_builder().build().map_err(|e| e.to_string())?,
        })
    }

    fn base_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(crate::app_title())
    }

    /// One-off client pinned to the addresses bashkit's SSRF precheck
    /// validated (reqwest only supports DNS overrides per client). Inert while
    /// proxied — the proxy resolves instead — the same trade-off bashkit's
    /// built-in transport makes.
    fn pinned_client(
        request: &bashkit::HttpTransportRequest,
    ) -> Result<reqwest::Client, bashkit::HttpTransportError> {
        let url = reqwest::Url::parse(&request.url)
            .map_err(|e| bashkit::HttpTransportError::Transport(e.to_string()))?;
        let host = url.host_str().ok_or_else(|| {
            bashkit::HttpTransportError::Transport("request URL has no host".to_string())
        })?;
        let port = url.port_or_known_default().unwrap_or(80);
        let addrs: Vec<SocketAddr> = request
            .pinned_addrs
            .iter()
            .map(|ip| SocketAddr::new(*ip, port))
            .collect();
        Self::base_builder()
            .resolve_to_addrs(host, &addrs)
            .build()
            .map_err(|e| bashkit::HttpTransportError::Transport(e.to_string()))
    }

    fn map_error(error: reqwest::Error) -> bashkit::HttpTransportError {
        if error.is_timeout() {
            bashkit::HttpTransportError::Timeout
        } else {
            bashkit::HttpTransportError::Transport(error.to_string())
        }
    }

    /// Stream the body, stopping at `max` instead of buffering past it
    /// (bashkit re-checks the cap after return).
    async fn read_body(
        mut response: reqwest::Response,
        max: usize,
    ) -> Result<Vec<u8>, bashkit::HttpTransportError> {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Self::map_error)? {
            if body.len() + chunk.len() > max {
                return Err(bashkit::HttpTransportError::TooLarge(format!(
                    "exceeded {max} bytes limit"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[bashkit::async_trait]
impl bashkit::HttpTransport for SystemProxyTransport {
    async fn execute(
        &self,
        request: bashkit::HttpTransportRequest,
    ) -> Result<bashkit::HttpResponse, bashkit::HttpTransportError> {
        use bashkit::HttpMethod::*;
        let method = match request.method {
            Get => reqwest::Method::GET,
            Post => reqwest::Method::POST,
            Put => reqwest::Method::PUT,
            Delete => reqwest::Method::DELETE,
            Head => reqwest::Method::HEAD,
            Patch => reqwest::Method::PATCH,
        };

        // SSRF (TM-NET-023): pin the dial to bashkit-validated addresses so
        // DNS rebinding can't slip another address through.
        let client = if request.pinned_addrs.is_empty() {
            self.client.clone()
        } else {
            Self::pinned_client(&request)?
        };

        let mut builder = client
            .request(method, &request.url)
            .timeout(request.timeout);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder.send().await.map_err(Self::map_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        // Fail fast on declared oversize bodies before buffering.
        if let Some(len) = response.content_length()
            && usize::try_from(len).unwrap_or(usize::MAX) > request.max_response_bytes
        {
            return Err(bashkit::HttpTransportError::TooLarge(format!(
                "{len} bytes (max: {} bytes)",
                request.max_response_bytes
            )));
        }
        let body = Self::read_body(response, request.max_response_bytes).await?;
        Ok(bashkit::HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// Sandbox transport set by `adopt_system_proxy`; unset means curl/wget use
/// bashkit's built-in direct transport.
static EGRESS: OnceLock<Arc<dyn bashkit::HttpTransport>> = OnceLock::new();

pub fn system_proxy_transport() -> Option<Arc<dyn bashkit::HttpTransport>> {
    wait_probe();
    EGRESS.get().cloned()
}
