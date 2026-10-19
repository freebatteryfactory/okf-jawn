//! Deployment settings of the service and of the offline commands, read once at startup and
//! refused by name before anything is touched.
//!
//! Every setting is an environment variable (server-m2-design.md section 1.2). Each is read
//! exactly as set: a value with surrounding whitespace is refused, and so is one that is present
//! and empty, so nothing is silently read as unset. A refusal is `InvalidInput` whose `field` is
//! the setting's name.
//!
//! - `OKF_BIND` and `OKF_SANDBOX_BIND` are IP socket addresses (`ip:port`, never a host name).
//!   The sandbox listens beside the application, by default on the same address and the next
//!   port (port 0 stays 0: any free port), and never on the application's own address.
//! - `OKF_PUBLIC_ORIGIN` and `OKF_SANDBOX_ORIGIN` are written exactly as a browser writes an
//!   origin (the text must equal its parsed origin's ASCII serialization), so the `Origin` the
//!   identity middleware compares and the `Host` the guards compare are what browsers send.
//!   Plain `http` is accepted only for a loopback host (SPEC section 2: remote endpoints require
//!   HTTPS) and never in hosted mode. Locally each defaults to `http://` and its bound address
//!   when that address is loopback, and is required otherwise; hosted, both are required.
//! - `OKF_ALLOWED_HOSTS` replaces the default host list (the public origin's authority) with an
//!   exact list; an empty entry is refused, so an empty `Host` header can never match.
//! - The sandbox origin is never the application's: not the same origin, not one of the hosts
//!   the application answers, and of the same scheme (an https page cannot frame an http one).
//! - Hosted, the AuthKit redirect URI is exactly `{public origin}/auth/callback` and the MCP
//!   resource exactly `{public origin}/mcp`: the service is served at its origin's root.
//! - Every path setting is absolute. The data directory defaults to the per-user local data
//!   directory, never to the working directory (a server the relay starts inherits its client's);
//!   the UI build, the MCP App bundle and the conversion child default to beside the executable.
//! - `OKF_CONVERTER_MEMORY_LIMIT_BYTES` is the conversion child's cap, ASCII digits, at least
//!   256 MiB; `OKF_TRUSTED_PROXIES` lists the proxies whose `X-Forwarded-For` is believed.
//!
//! The checks that need the bound addresses (a port-0 bind leaves the port open until then) run
//! in `Settings::resolve`, after binding and before the data directory is opened; the three
//! files are checked by `Settings::check_files`. The offline commands read only the mode and the
//! data directory (`offline_settings`): they open no listener and need no WorkOS credential.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use axum::http::uri::Authority;
use okf_jawn_contract::error::ApiError;
use url::{Host, Url};

use crate::config::{AuthConfig, AuthKind, invalid};

/// Where the defaults are on this machine; injected, so the rules are tested without depending
/// on the machine they run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// The directory of the running executable: the UI build, the MCP App bundle and the
    /// conversion child default to beside it. `None` when it cannot be known.
    pub exe_dir: Option<PathBuf>,
    /// The per-user local data directory (`directories::ProjectDirs::data_local_dir` of
    /// `okf-jawn`). `None` when the platform names none.
    pub user_data: Option<PathBuf>,
}

/// Every setting of the service, each checked alone and against the others that do not need a
/// bound address.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The authentication entry path and, hosted, its WorkOS settings.
    pub auth: AuthConfig,
    /// The application listener's address (`OKF_BIND`).
    pub bind: SocketAddr,
    /// The configured public origin (`OKF_PUBLIC_ORIGIN`); `None` derives it from the bound
    /// loopback address.
    pub public_origin: Option<String>,
    /// The configured hosts the application answers (`OKF_ALLOWED_HOSTS`); `None` is the public
    /// origin's authority.
    pub allowed_hosts: Option<Vec<String>>,
    /// The sandbox listener's address (`OKF_SANDBOX_BIND`).
    pub sandbox_bind: SocketAddr,
    /// The configured sandbox origin (`OKF_SANDBOX_ORIGIN`); `None` derives it from the bound
    /// loopback sandbox address.
    pub sandbox_origin: Option<String>,
    /// The data directory (`OKF_DATA_DIR`), absolute.
    pub data_dir: PathBuf,
    /// The UI build (`OKF_UI_DIR`), absolute.
    pub ui_dir: PathBuf,
    /// The MCP App bundle (`OKF_MCP_APP`), absolute.
    pub mcp_app: PathBuf,
    /// The conversion child (`OKF_CONVERTER`), absolute.
    pub converter: PathBuf,
    /// The conversion child's memory cap in bytes (`OKF_CONVERTER_MEMORY_LIMIT_BYTES`).
    pub converter_memory_limit_bytes: u64,
    /// The proxies whose `X-Forwarded-For` is believed (`OKF_TRUSTED_PROXIES`).
    pub trusted_proxies: Vec<IpAddr>,
}

/// The settings the bound addresses complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The application origin, as browsers write it.
    pub public_origin: String,
    /// The `Host` values the application answers; never empty, no empty entry.
    pub hosts: Vec<String>,
    /// The sandbox origin, as browsers write it.
    pub sandbox_origin: String,
    /// The sandbox origin's authority: the one `Host` the sandbox service answers.
    pub sandbox_host: String,
}

/// The application listener when `OKF_BIND` is unset (SPEC section 2).
pub const DEFAULT_BIND: &str = "127.0.0.1:7711";

/// The conversion child's memory cap when `OKF_CONVERTER_MEMORY_LIMIT_BYTES` is unset: 4 GiB,
/// above the 2,620,903,424-byte peak the Docling qualification measured with no limit, until
/// part (b) of `converter-worker-memory-ceiling` measures the windowed peak.
pub const DEFAULT_CONVERTER_MEMORY_LIMIT_BYTES: u64 = 4_294_967_296;

/// The smallest cap accepted: 256 MiB. Below it the cap classification's margin
/// (`okf_jawn_ingest::cap::CAP_MARGIN_MIN_BYTES`, 64 MiB) is a quarter of the cap or more.
pub const MIN_CONVERTER_MEMORY_LIMIT_BYTES: u64 = 268_435_456;

/// The conversion child's file name, without the platform's executable suffix.
pub const CONVERTER_PROGRAM: &str = "okf-jawn-convert";

impl Settings {
    /// Read every setting through `lookup` (the process environment at startup), with the
    /// defaults `places` gives.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming the first setting that is refused, in this order: the
    /// authentication settings, `OKF_BIND`, `OKF_SANDBOX_BIND`, `OKF_PUBLIC_ORIGIN`,
    /// `OKF_ALLOWED_HOSTS`, `OKF_SANDBOX_ORIGIN`, `OKF_DATA_DIR`, `OKF_UI_DIR`, `OKF_MCP_APP`,
    /// `OKF_CONVERTER`, `OKF_CONVERTER_MEMORY_LIMIT_BYTES`, `OKF_TRUSTED_PROXIES`.
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
        places: &Places,
    ) -> Result<Self, ApiError> {
        let auth = AuthConfig::from_lookup(&lookup)?;
        let hosted = auth.kind() == AuthKind::Hosted;
        let bind = match exact(&lookup, "OKF_BIND")? {
            Some(text) => socket_address("OKF_BIND", &text)?,
            None => socket_address("OKF_BIND", DEFAULT_BIND)?,
        };
        let sandbox_bind = match exact(&lookup, "OKF_SANDBOX_BIND")? {
            Some(text) => socket_address("OKF_SANDBOX_BIND", &text)?,
            None => beside(bind)?,
        };
        if sandbox_bind == bind && bind.port() != 0 {
            return Err(invalid(
                "OKF_SANDBOX_BIND",
                "must not be the application's own address (OKF_BIND)",
            ));
        }
        let public_origin = origin_setting(&lookup, "OKF_PUBLIC_ORIGIN", hosted, bind)?;
        let allowed_hosts = exact(&lookup, "OKF_ALLOWED_HOSTS")?
            .map(|text| host_list(&text))
            .transpose()?;
        let sandbox_origin = origin_setting(&lookup, "OKF_SANDBOX_ORIGIN", hosted, sandbox_bind)?;
        let data_dir = path_setting(&lookup, "OKF_DATA_DIR", places.user_data.clone())?;
        let beside_exe = |name: &str| places.exe_dir.as_ref().map(|dir| dir.join(name));
        let ui_dir = path_setting(&lookup, "OKF_UI_DIR", beside_exe("ui"))?;
        let mcp_app = path_setting(
            &lookup,
            "OKF_MCP_APP",
            beside_exe("mcp-apps").map(|dir| dir.join("app.html")),
        )?;
        let converter = path_setting(
            &lookup,
            "OKF_CONVERTER",
            beside_exe(&format!(
                "{CONVERTER_PROGRAM}{}",
                std::env::consts::EXE_SUFFIX
            )),
        )?;
        let converter_memory_limit_bytes = match exact(&lookup, "OKF_CONVERTER_MEMORY_LIMIT_BYTES")?
        {
            Some(text) => memory_limit(&text)?,
            None => DEFAULT_CONVERTER_MEMORY_LIMIT_BYTES,
        };
        let trusted_proxies = match exact(&lookup, "OKF_TRUSTED_PROXIES")? {
            Some(text) => proxy_list(&text)?,
            None => Vec::new(),
        };
        Ok(Self {
            auth,
            bind,
            public_origin,
            allowed_hosts,
            sandbox_bind,
            sandbox_origin,
            data_dir,
            ui_dir,
            mcp_app,
            converter,
            converter_memory_limit_bytes,
            trusted_proxies,
        })
    }

    /// Complete the settings with the addresses the two listeners were bound to, and run the
    /// checks that compare the origins and hosts.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming `OKF_SANDBOX_ORIGIN` when the sandbox origin is the
    /// application's, is one of its hosts, or has another scheme; and, hosted, naming
    /// `WORKOS_REDIRECT_URI` or `WORKOS_MCP_RESOURCE` when it is not under the public origin.
    pub fn resolve(&self, app: SocketAddr, sandbox: SocketAddr) -> Result<Resolved, ApiError> {
        let public_origin = self
            .public_origin
            .clone()
            .unwrap_or_else(|| format!("http://{app}"));
        let hosts = match &self.allowed_hosts {
            Some(hosts) => hosts.clone(),
            None => vec![authority_of(&public_origin).to_owned()],
        };
        let sandbox_origin = self
            .sandbox_origin
            .clone()
            .unwrap_or_else(|| format!("http://{sandbox}"));
        let sandbox_host = authority_of(&sandbox_origin).to_owned();
        if sandbox_origin == public_origin {
            return Err(invalid(
                "OKF_SANDBOX_ORIGIN",
                "must not be the application's own origin (OKF_PUBLIC_ORIGIN)",
            ));
        }
        if hosts
            .iter()
            .any(|host| host.eq_ignore_ascii_case(&sandbox_host))
        {
            return Err(invalid(
                "OKF_SANDBOX_ORIGIN",
                "must not name a host the application answers (OKF_ALLOWED_HOSTS)",
            ));
        }
        if scheme_of(&sandbox_origin) != scheme_of(&public_origin) {
            return Err(invalid(
                "OKF_SANDBOX_ORIGIN",
                "must have the scheme of OKF_PUBLIC_ORIGIN: a page cannot frame another scheme",
            ));
        }
        if let AuthConfig::Hosted(hosted) = &self.auth {
            if hosted.redirect_uri != format!("{public_origin}/auth/callback") {
                return Err(invalid(
                    "WORKOS_REDIRECT_URI",
                    "must be OKF_PUBLIC_ORIGIN followed by /auth/callback",
                ));
            }
            if hosted.resource != format!("{public_origin}/mcp") {
                return Err(invalid(
                    "WORKOS_MCP_RESOURCE",
                    "must be OKF_PUBLIC_ORIGIN followed by /mcp",
                ));
            }
        }
        Ok(Resolved {
            public_origin,
            hosts,
            sandbox_origin,
            sandbox_host,
        })
    }

    /// Check the three files the service serves or runs before anything is touched: the UI
    /// build's `index.html`, the MCP App bundle and the conversion child.
    ///
    /// # Errors
    /// Returns `InvalidInput` naming `OKF_UI_DIR`, `OKF_MCP_APP` or `OKF_CONVERTER` when its file
    /// is not a file.
    pub fn check_files(&self) -> Result<(), ApiError> {
        if !self.ui_dir.join("index.html").is_file() {
            return Err(invalid(
                "OKF_UI_DIR",
                "must name the UI build, a directory holding index.html",
            ));
        }
        if !self.mcp_app.is_file() {
            return Err(invalid("OKF_MCP_APP", "must name the built MCP App file"));
        }
        if !self.converter.is_file() {
            return Err(invalid(
                "OKF_CONVERTER",
                "must name the conversion child executable (okf-jawn-convert)",
            ));
        }
        Ok(())
    }
}

impl Places {
    /// This machine's places: the running executable's directory and the per-user local data
    /// directory of `okf-jawn` (Windows `%LOCALAPPDATA%\okf-jawn\data`, Linux
    /// `$XDG_DATA_HOME/okf-jawn` or `~/.local/share/okf-jawn`, macOS
    /// `~/Library/Application Support/okf-jawn`).
    #[must_use]
    pub fn of_this_machine() -> Self {
        Self {
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf)),
            user_data: directories::ProjectDirs::from("", "", "okf-jawn")
                .map(|dirs| dirs.data_local_dir().to_path_buf()),
        }
    }
}

/// What the offline commands read: the entry path, by the mode rule alone, and the data
/// directory.
///
/// # Errors
/// Returns `InvalidInput` naming `OKF_AUTH_MODE` or `OKF_DATA_DIR`.
pub fn offline_settings(
    lookup: impl Fn(&str) -> Option<String>,
    places: &Places,
) -> Result<(AuthKind, PathBuf), ApiError> {
    let kind = AuthKind::from_lookup(&lookup)?;
    let data_dir = path_setting(&lookup, "OKF_DATA_DIR", places.user_data.clone())?;
    Ok((kind, data_dir))
}

/// The value of `name` as set, refused when it is empty or carries surrounding whitespace.
fn exact(lookup: impl Fn(&str) -> Option<String>, name: &str) -> Result<Option<String>, ApiError> {
    match lookup(name) {
        None => Ok(None),
        Some(value) if value.is_empty() => Err(invalid(name, "is set but empty")),
        Some(value) if value.trim() != value => {
            Err(invalid(name, "must not begin or end with whitespace"))
        }
        Some(value) => Ok(Some(value)),
    }
}

/// An IP socket address, `ip:port`; a host name is refused.
fn socket_address(name: &str, text: &str) -> Result<SocketAddr, ApiError> {
    SocketAddr::from_str(text).map_err(|_| {
        invalid(
            name,
            "must be an IP address and a port, such as 127.0.0.1:7711 or [::1]:7711",
        )
    })
}

/// The sandbox's default address: `bind`'s IP and the next port; port 0 stays 0.
fn beside(bind: SocketAddr) -> Result<SocketAddr, ApiError> {
    if bind.port() == 0 {
        return Ok(bind);
    }
    let port = bind.port().checked_add(1).ok_or_else(|| {
        invalid(
            "OKF_SANDBOX_BIND",
            "must be set when OKF_BIND's port is 65535 (it defaults to the next port)",
        )
    })?;
    Ok(SocketAddr::new(bind.ip(), port))
}

/// An origin setting: checked when set; when unset, derived later from a loopback `bind`
/// locally, and refused otherwise.
fn origin_setting(
    lookup: impl Fn(&str) -> Option<String>,
    name: &str,
    hosted: bool,
    bind: SocketAddr,
) -> Result<Option<String>, ApiError> {
    match exact(lookup, name)? {
        Some(text) => {
            let url = canonical_origin(name, &text)?;
            if url.scheme() == "http" {
                if hosted {
                    return Err(invalid(name, "must be https in workos mode"));
                }
                if !loopback_host(&url) {
                    return Err(invalid(
                        name,
                        "must be https: plain http is for a loopback host only",
                    ));
                }
            }
            Ok(Some(text))
        }
        None if hosted => Err(invalid(name, "is required in workos mode")),
        None if bind.ip().is_loopback() => Ok(None),
        None => Err(invalid(
            name,
            "is required when its listener's address is not a loopback address",
        )),
    }
}

/// `text` parsed, when it is an `http` or `https` origin written exactly as its ASCII
/// serialization: no path, query, fragment or user information, a lower-case host, and no
/// default port spelled out.
fn canonical_origin(name: &str, text: &str) -> Result<Url, ApiError> {
    let refused = || {
        invalid(
            name,
            "must be an http or https origin written as a browser writes it: scheme, host and \
             optional port, lower case, no default port, no trailing slash, path, query or \
             user information",
        )
    };
    let url = Url::parse(text).map_err(|_| refused())?;
    if !matches!(url.scheme(), "http" | "https") || url.origin().ascii_serialization() != text {
        return Err(refused());
    }
    Ok(url)
}

/// Whether `url`'s host is this computer: `localhost`, an IPv4 loopback address or `[::1]`.
fn loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// The authority of a checked origin: everything after `scheme://`.
fn authority_of(origin: &str) -> &str {
    origin
        .split_once("://")
        .map_or(origin, |(_, authority)| authority)
}

/// The scheme of a checked origin.
fn scheme_of(origin: &str) -> &str {
    origin.split_once("://").map_or("", |(scheme, _)| scheme)
}

/// `OKF_ALLOWED_HOSTS`: comma-separated authorities (`host` or `host:port`), none empty.
fn host_list(text: &str) -> Result<Vec<String>, ApiError> {
    text.split(',')
        .map(|entry| {
            let plain = !entry.is_empty()
                && entry
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'@' | b'/' | b'*'));
            if plain && Authority::from_str(entry).is_ok() {
                Ok(entry.to_owned())
            } else {
                Err(invalid(
                    "OKF_ALLOWED_HOSTS",
                    "must be comma-separated hosts (host or host:port), none empty, with no \
                     whitespace, user information, path or wildcard",
                ))
            }
        })
        .collect()
}

/// A path setting: absolute when set; `default` when unset, refused when there is none.
fn path_setting(
    lookup: impl Fn(&str) -> Option<String>,
    name: &str,
    default: Option<PathBuf>,
) -> Result<PathBuf, ApiError> {
    match exact(lookup, name)? {
        Some(text) => {
            let path = PathBuf::from(text);
            if path.is_absolute() {
                Ok(path)
            } else {
                Err(invalid(
                    name,
                    "must be an absolute path: a relative one would depend on the working \
                     directory",
                ))
            }
        }
        None => default.ok_or_else(|| {
            invalid(
                name,
                "has no default on this machine (the place it defaults to is unknown); set it",
            )
        }),
    }
}

/// `OKF_CONVERTER_MEMORY_LIMIT_BYTES`: ASCII digits, at least the minimum.
fn memory_limit(text: &str) -> Result<u64, ApiError> {
    let name = "OKF_CONVERTER_MEMORY_LIMIT_BYTES";
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(name, "must be a byte count in ASCII digits"));
    }
    let limit = text
        .parse::<u64>()
        .map_err(|_| invalid(name, "must be a byte count that fits in 64 bits"))?;
    if limit < MIN_CONVERTER_MEMORY_LIMIT_BYTES {
        return Err(invalid(name, "must be at least 268435456 bytes (256 MiB)"));
    }
    Ok(limit)
}

/// `OKF_TRUSTED_PROXIES`: comma-separated IP addresses, none empty, no ranges.
fn proxy_list(text: &str) -> Result<Vec<IpAddr>, ApiError> {
    text.split(',')
        .map(|entry| {
            IpAddr::from_str(entry).map_err(|_| {
                invalid(
                    "OKF_TRUSTED_PROXIES",
                    "must be comma-separated IP addresses, none empty, no ranges or names",
                )
            })
        })
        .collect()
}
