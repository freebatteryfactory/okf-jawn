//! The composed service, started in process over a temporary data directory and real storage,
//! and reached over real sockets: liveness without a credential and only for its own host; every
//! declared transport inside the host guard; the launch link and the cookie scheme of the public
//! origin; one event log for sign-ins and refusals; the MCP challenge; the sandbox origin's own
//! listener; the page headers; a hosted composition that builds its sign-in URL; and a shutdown
//! that frees the listeners, the launch file and the data directory's lock.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::http::Request;
use futures_util::StreamExt as _;
use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::events::{EventKind, ListEventsResponse};
use okf_jawn_contract::import::{Job, JobState, Upload};
use okf_jawn_contract::workspace::{DownloadArtifact, Workspace};
use okf_jawn_server::launch::LAUNCH_FILE;
use okf_jawn_server::settings::{Places, Settings};
use okf_jawn_server::startup::{compose, serve};
use okf_jawn_storage::Storage;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

use check::{TestResult, err_of, some};

/// A temporary installation: a UI build, an App bundle, a conversion child and a data directory.
struct Installation {
    root: PathBuf,
}

/// A started service.
struct Running {
    origin: String,
    sandbox: String,
    launch: Option<String>,
    stop: CancellationToken,
    served: JoinHandle<Result<(), okf_jawn_contract::error::ApiError>>,
    data: PathBuf,
}

/// A test-owned binary; even a failed assertion stops and reaps it.
struct Binary {
    child: Child,
    lines: mpsc::Receiver<String>,
    errors: Arc<Mutex<String>>,
    started: Instant,
}

/// A signed-in test browser, sharing the real service's session and anti-CSRF cookie.
struct Browser {
    http: reqwest::Client,
    origin: String,
    cookie: String,
    csrf: String,
}

static NEXT: AtomicU64 = AtomicU64::new(0);

/// The sixteen declared transports' paths, with their parameters filled.
const TRANSPORT_PATHS: [(&str, &str); 16] = [
    (
        "PUT",
        "/api/workspaces/11111111-1111-4111-8111-111111111111/uploads/22222222-2222-4222-8222-222222222222/content",
    ),
    (
        "GET",
        "/api/workspaces/11111111-1111-4111-8111-111111111111/items/33333333-3333-4333-8333-333333333333/revisions/0000000000000000000000000000000000000000/objects/0000000000000000000000000000000000000000000000000000000000000000",
    ),
    (
        "GET",
        "/api/workspaces/11111111-1111-4111-8111-111111111111/artifacts/44444444-4444-4444-8444-444444444444",
    ),
    ("GET", "/api/artifacts/44444444-4444-4444-8444-444444444444"),
    ("GET", "/sandbox/a-capability"),
    (
        "GET",
        "/api/workspaces/11111111-1111-4111-8111-111111111111/events",
    ),
    ("GET", "/api/schemas/view"),
    ("GET", "/.well-known/oauth-protected-resource"),
    ("GET", "/auth/local?token=x"),
    ("GET", "/auth/login"),
    ("GET", "/auth/callback?state=x&code=y"),
    ("POST", "/auth/logout"),
    ("GET", "/healthz"),
    ("POST", "/mcp"),
    ("GET", "/mcp"),
    ("DELETE", "/mcp"),
];

impl Installation {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "okf-jawn-startup-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(root.join("ui"))?;
        std::fs::write(
            root.join("ui").join("index.html"),
            "<!doctype html><title>workspace</title>",
        )?;
        std::fs::write(root.join("app.html"), "<!doctype html><title>app</title>")?;
        // Nothing converts in these tests; startup only checks the child is a file.
        std::fs::write(root.join("okf-jawn-convert"), "")?;
        Ok(Self { root })
    }

    fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// The local settings of this installation, listening on any free loopback port.
    fn local(&self) -> Result<BTreeMap<String, String>, String> {
        let text = |path: &Path| {
            path.to_str()
                .map(str::to_owned)
                .ok_or_else(|| "a path that is not UTF-8".to_owned())
        };
        Ok(BTreeMap::from([
            ("OKF_BIND".to_owned(), "127.0.0.1:0".to_owned()),
            ("OKF_DATA_DIR".to_owned(), text(&self.data())?),
            ("OKF_UI_DIR".to_owned(), text(&self.root.join("ui"))?),
            ("OKF_MCP_APP".to_owned(), text(&self.root.join("app.html"))?),
            (
                "OKF_CONVERTER".to_owned(),
                text(&self.root.join("okf-jawn-convert"))?,
            ),
        ]))
    }

    /// Hosted configuration uses reserved example hosts and a fake key; no request calls WorkOS.
    fn hosted(&self) -> Result<BTreeMap<String, String>, String> {
        let mut settings = self.local()?;
        for (name, value) in [
            ("OKF_AUTH_MODE", "workos"),
            ("WORKOS_CLIENT_ID", "client_01"),
            ("WORKOS_API_KEY", "not-a-real-api-key"),
            ("WORKOS_ISSUER", "https://auth.example.test"),
            ("WORKOS_MCP_RESOURCE", "https://okf.example.test/mcp"),
            (
                "WORKOS_REDIRECT_URI",
                "https://okf.example.test/auth/callback",
            ),
            ("OKF_PUBLIC_ORIGIN", "https://okf.example.test"),
            ("OKF_SANDBOX_ORIGIN", "https://sandbox.example.test"),
        ] {
            settings.insert(name.to_owned(), value.to_owned());
        }
        Ok(settings)
    }

    fn spawn_binary<K: AsRef<OsStr>, V: AsRef<OsStr>>(
        &self,
        settings: impl IntoIterator<Item = (K, V)>,
        args: &[&str],
        close_stdout: bool,
    ) -> Result<Binary, Box<dyn std::error::Error>> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_okf-jawn-server"));
        command
            .args(args)
            .env_clear()
            .envs(settings)
            .current_dir(&self.root);
        // Keep platform/runtime paths, without inheriting any deployment credential or setting.
        for name in ["SYSTEMROOT", "WINDIR", "PATH", "TMP", "TEMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000);
        }
        let started = Instant::now();
        let mut child = command.spawn()?;
        let (sender, lines) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            if close_stdout {
                drop(stdout);
            } else {
                let _stdout_reader = std::thread::spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                });
            }
        }
        let errors = Arc::new(Mutex::new(String::new()));
        if let Some(mut stderr) = child.stderr.take() {
            let captured = Arc::clone(&errors);
            let _stderr_reader = std::thread::spawn(move || {
                let mut text = String::new();
                let _read = stderr.read_to_string(&mut text);
                if let Ok(mut errors) = captured.lock() {
                    *errors = text;
                }
            });
        }
        Ok(Binary {
            child,
            lines,
            errors,
            started,
        })
    }

    /// Start the service with `settings`; wait until it has printed its listening lines.
    async fn start(
        &self,
        settings: BTreeMap<String, String>,
    ) -> Result<Running, Box<dyn std::error::Error>> {
        let printed = Arc::new(Mutex::new(Vec::<String>::new()));
        let lines = Arc::clone(&printed);
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let served = tokio::spawn(async move {
            let print = move |line: &str| {
                if let Ok(mut lines) = lines.lock() {
                    lines.push(line.to_owned());
                }
            };
            serve(
                |name| settings.get(name).cloned(),
                &Places {
                    exe_dir: None,
                    user_data: None,
                },
                &print,
                stopping,
            )
            .await
        });
        let deadline = some(
            tokio::time::Instant::now().checked_add(Duration::from_secs(120)),
            "the startup deadline",
        )?;
        loop {
            let lines = printed.lock().map_err(|_| "poisoned")?.clone();
            let value = |key: &str| {
                lines
                    .iter()
                    .find_map(|line| line.strip_prefix(&format!("okf-jawn-server: {key} ")))
                    .map(str::to_owned)
            };
            if let (Some(origin), Some(sandbox)) = (value("listening"), value("sandbox")) {
                // A local service prints its launch link right after the two origins.
                let hosted = !origin.starts_with("http://127.0.0.1");
                if hosted || value("launch").is_some() {
                    return Ok(Running {
                        origin,
                        sandbox,
                        launch: value("launch"),
                        stop,
                        served,
                        data: self.data(),
                    });
                }
            }
            if served.is_finished() {
                return Err(
                    format!("the service ended before it listened: {:?}", served.await?).into(),
                );
            }
            if tokio::time::Instant::now() >= deadline {
                stop.cancel();
                served.abort();
                let _stopped = served.await;
                return Err("the service printed no listening lines in two minutes".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Drop for Installation {
    fn drop(&mut self) {
        // Best effort: a leftover directory under the temporary directory harms nothing.
        let _removed = std::fs::remove_dir_all(&self.root);
    }
}

impl Running {
    fn authority(&self) -> &str {
        self.origin
            .split_once("://")
            .map_or("", |(_, authority)| authority)
    }

    async fn stop(mut self) -> Result<PathBuf, Box<dyn std::error::Error>> {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(60), &mut self.served).await???;
        Ok(self.data.clone())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Binary {
    fn line(&self) -> Result<String, Box<dyn std::error::Error>> {
        let left = Duration::from_secs(45).saturating_sub(self.started.elapsed());
        Ok(self.lines.recv_timeout(left)?)
    }

    fn exit(&mut self) -> Result<ExitStatus, Box<dyn std::error::Error>> {
        let until = some(
            Instant::now().checked_add(Duration::from_secs(10)),
            "the refusing binary's exit deadline",
        )?;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= until {
                return Err("the refusing binary did not exit in ten seconds".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stderr(&self) -> Result<String, Box<dyn std::error::Error>> {
        // The reader finishes at EOF after exit; wait for its captured diagnostic, not a fixed sleep.
        let until = some(
            Instant::now().checked_add(Duration::from_secs(5)),
            "the exited binary's diagnostic deadline",
        )?;
        loop {
            let errors = self.errors.lock().map_err(|_| "poisoned")?.clone();
            if !errors.is_empty() {
                return Ok(errors);
            }
            if Instant::now() >= until {
                return Err("the exited binary wrote no diagnostic".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Binary {
    fn drop(&mut self) {
        let _killed = self.child.kill();
        let _reaped = self.child.wait();
    }
}

impl Browser {
    async fn sign_in(running: &Running) -> Result<Self, Box<dyn std::error::Error>> {
        let http = client()?;
        let signed = http
            .get(some(running.launch.as_ref(), "the launch link")?)
            .send()
            .await?;
        assert_eq!(signed.status(), StatusCode::FOUND);
        let set = cookies(signed.headers());
        Ok(Self {
            http,
            origin: running.origin.clone(),
            cookie: format!(
                "okf-session={}",
                some(set.get("okf-session"), "the session")?
            ),
            csrf: some(set.get("okf-csrf"), "the anti-CSRF cookie")?.clone(),
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.origin))
            .header("cookie", &self.cookie)
            .header("origin", &self.origin)
            .header("x-okf-csrf", &self.csrf)
    }

    async fn workspace(&self) -> Result<Workspace, Box<dyn std::error::Error>> {
        Ok(self.request(reqwest::Method::POST, "/api/workspaces/create-workspace")
            .json(&json!({ "name": "Transfers", "description": "", "idempotency_key": "00000000-0000-4000-8000-000000000010" }))
            .send().await?.error_for_status()?.json().await?)
    }

    async fn artifact(
        &self,
        workspace: &Workspace,
        installation: bool,
    ) -> Result<DownloadArtifact, Box<dyn std::error::Error>> {
        let (path, request, poll) = if installation {
            (
                "/api/workspaces/backup-installation",
                json!({ "idempotency_key": "00000000-0000-4000-8000-000000000011" }),
                "/api/imports/get-tenant-job",
            )
        } else {
            (
                "/api/workspaces/backup-workspace",
                json!({ "workspace_id": workspace.id, "idempotency_key": "00000000-0000-4000-8000-000000000012" }),
                "/api/imports/get-job",
            )
        };
        let accepted = self
            .request(reqwest::Method::POST, path)
            .json(&request)
            .send()
            .await?;
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        let mut job: Job = accepted.json().await?;
        let until = some(
            tokio::time::Instant::now().checked_add(Duration::from_secs(30)),
            "the backup deadline",
        )?;
        loop {
            match job.state {
                JobState::Succeeded => {
                    return Ok(some(job.artifact, "the completed backup artifact")?);
                }
                JobState::Failed | JobState::Cancelled => {
                    return Err("the composed backup failed".into());
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= until {
                return Err("the backup did not complete in thirty seconds".into());
            }
            let query = if installation {
                json!({ "job_id": job.id })
            } else {
                json!({ "job_id": job.id, "workspace_id": workspace.id })
            };
            job = self
                .request(reqwest::Method::POST, poll)
                .json(&query)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// The relay's existing 45-second start budget, measured from spawning the actual binary.
async fn alive_before_relay_deadline(origin: &str, started: Instant) -> TestResult {
    let http = client()?;
    loop {
        if let Ok(response) = http
            .get(format!("{origin}/healthz"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            && response.status() == StatusCode::OK
        {
            assert!(started.elapsed() < Duration::from_secs(45));
            return Ok(());
        }
        if started.elapsed() >= Duration::from_secs(45) {
            return Err("the binary missed the relay's startup deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A client that follows no redirect and asks no proxy.
fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
}

/// The cookies a response sets, by name: their values.
fn cookies(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|cookie| cookie.split(';').next())
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

#[tokio::test]
async fn a_local_service_signs_in_and_records_in_one_log() -> TestResult {
    let installation = Installation::new()?;
    let running = installation.start(installation.local()?).await?;
    let http = client()?;
    assert!(running.origin.starts_with("http://127.0.0.1:"));
    assert!(running.sandbox.starts_with("http://127.0.0.1:"));
    // Liveness, without a credential.
    let alive = http
        .get(format!("{}/healthz", running.origin))
        .send()
        .await?;
    assert_eq!(alive.status(), StatusCode::OK);
    // The launch link, written in the data directory too.
    let launch = some(running.launch.clone(), "the launch link")?;
    let file = std::fs::read_to_string(running.data.join(LAUNCH_FILE))?;
    assert!(
        file.trim_end() == launch,
        "the launch file differs from the announced link"
    );
    let signed = http.get(&launch).send().await?;
    assert_eq!(signed.status(), StatusCode::FOUND);
    let set = cookies(signed.headers());
    let session = some(set.get("okf-session"), "the http session cookie")?;
    let csrf = some(set.get("okf-csrf"), "the http anti-CSRF cookie")?;
    assert!(!set.contains_key("__Host-okf-session"));
    let cookie = format!("okf-session={session}");
    let post = |path: &str, body: Value, with_csrf: bool| {
        let mut request = http
            .post(format!("{}{path}", running.origin))
            .header("cookie", &cookie)
            .header("origin", &running.origin)
            .json(&body);
        if with_csrf {
            request = request.header("x-okf-csrf", csrf.as_str());
        }
        request.send()
    };
    let listed = post(
        "/api/workspaces/list-workspaces",
        json!({ "page": { "limit": 10 } }),
        true,
    )
    .await?;
    assert_eq!(listed.status(), StatusCode::OK);
    let created = post(
        "/api/workspaces/create-workspace",
        json!({
            "name": "Composed",
            "description": "",
            "idempotency_key": "00000000-0000-4000-8000-000000000001",
        }),
        true,
    )
    .await?;
    assert_eq!(created.status(), StatusCode::OK);
    // A write without its anti-CSRF header is refused, and recorded in the same log.
    let refused = post(
        "/api/workspaces/list-workspaces",
        json!({ "page": { "limit": 10 } }),
        false,
    )
    .await?;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let events = post(
        "/api/events/list-tenant-events",
        json!({ "page": { "limit": 100 } }),
        true,
    )
    .await?;
    assert_eq!(events.status(), StatusCode::OK);
    let events: ListEventsResponse = events.json().await?;
    let kinds: Vec<EventKind> = events.events.into_iter().map(|event| event.kind).collect();
    assert!(kinds.contains(&EventKind::SignedIn), "{kinds:?}");
    assert!(kinds.contains(&EventKind::PermissionDenied), "{kinds:?}");
    // The MCP endpoint wants a bearer.
    let mcp = http.post(format!("{}/mcp", running.origin)).send().await?;
    assert_eq!(mcp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        mcp.headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("Bearer"))
    );
    let data = running.stop().await?;
    // The lock is free, and the launch file gone.
    drop(Storage::open(&data)?);
    assert!(!data.join(LAUNCH_FILE).exists());
    Ok(())
}

#[tokio::test]
async fn only_the_service_s_own_host_is_answered_on_every_declared_path() -> TestResult {
    let installation = Installation::new()?;
    let running = installation.start(installation.local()?).await?;
    let http = client()?;
    let own = http
        .get(format!("{}/healthz", running.origin))
        .header("host", running.authority())
        .send()
        .await?;
    assert_eq!(own.status(), StatusCode::OK);
    let port = running
        .authority()
        .rsplit_once(':')
        .map_or("", |(_, port)| port);
    let foreign = HeaderValue::from_str(&format!("localhost:{port}"))?;
    let operations = [
        "/api/workspaces/list-workspaces",
        "/api/items/create-folder",
        "/api/health/get-readiness",
    ];
    let mut paths: Vec<(&str, &str)> = TRANSPORT_PATHS.to_vec();
    paths.extend(operations.iter().map(|path| ("POST", *path)));
    for (method, path) in paths {
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let response = http
            .request(method.clone(), format!("{}{path}", running.origin))
            .header("host", foreign.clone())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
    running.stop().await?;
    Ok(())
}

#[tokio::test]
async fn the_sandbox_has_its_own_listener_and_the_pages_their_headers() -> TestResult {
    let installation = Installation::new()?;
    let running = installation.start(installation.local()?).await?;
    let http = client()?;
    let sandboxed = http
        .get(format!("{}/sandbox/not-a-capability", running.sandbox))
        .send()
        .await?;
    assert_eq!(sandboxed.status(), StatusCode::NOT_FOUND);
    assert!(
        sandboxed
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|policy| policy.starts_with("sandbox;"))
    );
    let liveness = http
        .get(format!("{}/healthz", running.sandbox))
        .send()
        .await?;
    assert_eq!(liveness.status(), StatusCode::NOT_FOUND);
    let on_app = http
        .get(format!("{}/sandbox/not-a-capability", running.origin))
        .send()
        .await?;
    assert_eq!(on_app.status(), StatusCode::NOT_FOUND);
    let page = http.get(format!("{}/", running.origin)).send().await?;
    assert_eq!(page.status(), StatusCode::OK);
    let policy = page
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        policy.contains(&format!("frame-src {};", running.sandbox)),
        "{policy}"
    );
    let json_path = http
        .get(format!("{}/api/nothing", running.origin))
        .send()
        .await?;
    assert_eq!(json_path.status(), StatusCode::NOT_FOUND);
    assert!(json_path.headers().get("content-security-policy").is_none());
    running.stop().await?;
    Ok(())
}

#[tokio::test]
async fn a_hosted_service_begins_its_sign_in_at_authkit_and_publishes_its_metadata() -> TestResult {
    let installation = Installation::new()?;
    let values = installation.hosted()?;
    let settings = Settings::from_lookup(|name| values.get(name).cloned(), &no_places())?;
    let resolved = settings.resolve("127.0.0.1:7711".parse()?, "127.0.0.1:7712".parse()?)?;
    let storage = Storage::open(&installation.data())?;
    let composed = compose(&settings, &resolved, &storage)?;
    assert!(composed.launch.is_none());
    let login = composed
        .app
        .clone()
        .oneshot(
            Request::get("/auth/login")
                .header("host", "okf.example.test")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(login.status(), StatusCode::FOUND);
    let location = some(login.headers().get("location"), "the AuthKit redirect")?.to_str()?;
    let location = reqwest::Url::parse(location)?;
    assert_eq!(location.host_str(), Some("api.workos.com"));
    assert_eq!(location.path(), "/user_management/authorize");
    let query: BTreeMap<_, _> = location.query_pairs().collect();
    assert_eq!(query.get("provider").map(AsRef::as_ref), Some("authkit"));
    let metadata = composed
        .app
        .clone()
        .oneshot(
            Request::get("/.well-known/oauth-protected-resource")
                .header("host", "okf.example.test")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(metadata.status(), StatusCode::OK);
    let metadata: Value = serde_json::from_slice(&to_bytes(metadata.into_body(), 16_384).await?)?;
    assert_eq!(
        metadata.get("resource"),
        Some(&json!("https://okf.example.test/mcp"))
    );
    assert_eq!(
        metadata.get("authorization_servers"),
        Some(&json!(["https://auth.example.test"]))
    );
    composed.runtime.shutdown().await;
    Ok(())
}

fn no_places() -> Places {
    Places {
        exe_dir: None,
        user_data: None,
    }
}

/// Design section 1.3 puts every fallible outbound-client check before data-directory changes.
#[tokio::test]
async fn an_invalid_outbound_key_is_refused_before_the_data_directory_is_opened() -> TestResult {
    let installation = Installation::new()?;
    let mut settings = installation.hosted()?;
    settings.insert("WORKOS_API_KEY".to_owned(), "invalid\nheader".to_owned());
    let error = err_of(
        tokio::time::timeout(
            Duration::from_secs(10),
            serve(
                |name| settings.get(name).cloned(),
                &no_places(),
                &|_| {},
                CancellationToken::new(),
            ),
        )
        .await?,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("WORKOS_API_KEY"));
    assert!(
        !installation.data().exists(),
        "preflight modified the data directory"
    );
    Ok(())
}

#[tokio::test]
async fn a_busy_listener_is_refused_before_the_data_directory_is_opened() -> TestResult {
    let installation = Installation::new()?;
    let held = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mut settings = installation.local()?;
    settings.insert("OKF_BIND".to_owned(), held.local_addr()?.to_string());
    settings.insert("OKF_SANDBOX_BIND".to_owned(), "127.0.0.1:0".to_owned());
    let error = err_of(
        serve(
            |name| settings.get(name).cloned(),
            &no_places(),
            &|_| {},
            CancellationToken::new(),
        )
        .await,
    )?;
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("OKF_BIND"));
    assert!(!installation.data().exists());
    Ok(())
}

#[tokio::test]
async fn local_session_cookie_names_follow_the_configured_https_origin() -> TestResult {
    // Design section 1.8(3): the public origin's scheme, rather than the local transport, rules.
    let installation = Installation::new()?;
    let mut values = installation.local()?;
    values.insert(
        "OKF_PUBLIC_ORIGIN".to_owned(),
        "https://okf.example.test".to_owned(),
    );
    values.insert(
        "OKF_SANDBOX_ORIGIN".to_owned(),
        "https://sandbox.example.test".to_owned(),
    );
    let settings = Settings::from_lookup(|name| values.get(name).cloned(), &no_places())?;
    let resolved = settings.resolve("127.0.0.1:7711".parse()?, "127.0.0.1:7712".parse()?)?;
    let storage = Storage::open(&installation.data())?;
    let composed = compose(&settings, &resolved, &storage)?;
    let launch = some(composed.launch.as_ref(), "the local launch link")?.publish_first()?;
    let target = reqwest::Url::parse(&launch)?;
    let response = composed
        .app
        .clone()
        .oneshot(
            Request::get(format!(
                "{}?{}",
                target.path(),
                target.query().unwrap_or_default()
            ))
            .header("host", "okf.example.test")
            .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FOUND);
    let set = cookies(response.headers());
    assert!(set.contains_key("__Host-okf-session"));
    assert!(set.contains_key("__Host-okf-csrf"));
    assert!(!set.contains_key("okf-session"));
    assert!(!set.contains_key("okf-csrf"));
    composed.runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn shutdown_closes_both_listeners_and_an_open_event_stream() -> TestResult {
    // Design section 1.8(8): real sockets must close and the real storage lock must be released.
    let installation = Installation::new()?;
    let running = installation.start(installation.local()?).await?;
    let http = client()?;
    let signed = http
        .get(some(running.launch.as_ref(), "the launch link")?)
        .send()
        .await?;
    let set = cookies(signed.headers());
    let session = some(set.get("okf-session"), "the session cookie")?;
    let csrf = some(set.get("okf-csrf"), "the anti-CSRF cookie")?;
    let cookie = format!("okf-session={session}");
    let created: Value = http.post(format!("{}/api/workspaces/create-workspace", running.origin))
        .header("cookie", &cookie).header("origin", &running.origin).header("x-okf-csrf", csrf)
        .json(&json!({ "name": "Shutdown", "description": "", "idempotency_key": "00000000-0000-4000-8000-000000000002" }))
        .send().await?.error_for_status()?.json().await?;
    let workspace = some(
        created.get("id").and_then(Value::as_str),
        "the workspace id",
    )?;
    let events = http
        .get(format!(
            "{}/api/workspaces/{workspace}/events",
            running.origin
        ))
        .header("cookie", &cookie)
        .send()
        .await?
        .error_for_status()?;
    let app_origin = running.origin.clone();
    let sandbox_origin = running.sandbox.clone();
    let data = running.stop().await?;
    let mut body = events.bytes_stream();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(bytes) = body.next().await {
            let _bytes = bytes?;
        }
        Ok::<(), reqwest::Error>(())
    })
    .await??;
    assert!(
        http.get(format!("{app_origin}/healthz"))
            .send()
            .await
            .is_err()
    );
    assert!(
        http.get(format!("{sandbox_origin}/healthz"))
            .send()
            .await
            .is_err()
    );
    drop(Storage::open(&data)?);
    assert!(!data.join(LAUNCH_FILE).exists());
    Ok(())
}

#[tokio::test]
async fn composed_uploads_stream_bytes_and_refuse_an_oversized_body_without_appending() -> TestResult
{
    // M1.4c composition rule N-r: the actual upload adapter, rather than a route double, cuts bytes.
    let installation = Installation::new()?;
    let running = installation.start(installation.local()?).await?;
    let browser = Browser::sign_in(&running).await?;
    let workspace = browser.workspace().await?;
    let upload: Upload = browser.request(reqwest::Method::POST, "/api/imports/create-upload")
        .json(&json!({ "workspace_id": workspace.id, "filename": "report.txt", "relative_path": "", "size": "3", "idempotency_key": "00000000-0000-4000-8000-000000000013" }))
        .send().await?.error_for_status()?.json().await?;
    let chunks = futures_util::stream::iter([Ok::<_, std::io::Error>("ab"), Ok("cd")]);
    let too_large = browser
        .request(reqwest::Method::PUT, &upload.upload_path)
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await?;
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let error: okf_jawn_contract::error::ApiError = too_large.json().await?;
    assert_eq!(error.code, ErrorCode::TooLarge);
    let held: Upload = browser
        .request(reqwest::Method::POST, "/api/imports/get-upload")
        .json(&json!({ "workspace_id": workspace.id, "upload_id": upload.id }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(held.received_bytes, "0");
    let chunks = futures_util::stream::iter([Ok::<_, std::io::Error>("a"), Ok("bc")]);
    let held: Upload = browser
        .request(reqwest::Method::PUT, &upload.upload_path)
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(held.received_bytes, "3");
    assert!(!held.complete);
    running.stop().await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_a_stalled_upload_and_releases_the_storage_lock() -> TestResult {
    // Design section 1.6: an in-flight request gets five seconds, then cannot retain storage.
    // A literal incomplete HTTP body keeps the real upload adapter awaiting its next byte.
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let installation = Installation::new()?;
    let mut running = installation.start(installation.local()?).await?;
    let browser = Browser::sign_in(&running).await?;
    let workspace = browser.workspace().await?;
    let upload: Upload = browser.request(reqwest::Method::POST, "/api/imports/create-upload")
        .json(&json!({ "workspace_id": workspace.id, "filename": "stalled.txt", "relative_path": "", "size": "3", "idempotency_key": "00000000-0000-4000-8000-000000000015" }))
        .send().await?.error_for_status()?.json().await?;
    let mut socket = tokio::net::TcpStream::connect(running.authority()).await?;
    socket.write_all(format!(
        "PUT {} HTTP/1.1\r\nHost: {}\r\nCookie: {}\r\nOrigin: {}\r\nx-okf-csrf: {}\r\nContent-Length: 3\r\nExpect: 100-continue\r\n\r\n",
        upload.upload_path, running.authority(), browser.cookie, browser.origin, browser.csrf,
    ).as_bytes()).await?;
    let mut continued = vec![0; b"HTTP/1.1 100 Continue\r\n\r\n".len()];
    tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut continued)).await??;
    assert_eq!(continued, b"HTTP/1.1 100 Continue\r\n\r\n");
    socket.write_all(b"a").await?;
    running.stop.cancel();
    tokio::time::timeout(Duration::from_secs(10), &mut running.served).await???;
    // Keep the client socket open while reclaiming the lock: closing it would hide a detached
    // server connection. Reopening must succeed without waiting for a client to finish its body.
    drop(Storage::open(&running.data)?);
    assert!(!running.data.join(LAUNCH_FILE).exists());
    let mut tail = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut tail)).await??;
    Ok(())
}

#[tokio::test]
async fn composed_backups_download_for_the_owner_and_are_refused_to_a_connector() -> TestResult {
    // SPEC section 12: both artifact routes must use kind/route authorization and real archives.
    let installation = Installation::new()?;
    let running = installation.start(installation.local()?).await?;
    let browser = Browser::sign_in(&running).await?;
    let workspace = browser.workspace().await?;
    let connector: okf_jawn_contract::access::IssuedConnector = browser
        .request(reqwest::Method::POST, "/api/session/create-connector")
        .json(&json!({ "label": "reader", "workspace_ids": [workspace.id], "allow_propose": false, "idempotency_key": "00000000-0000-4000-8000-000000000014" }))
        .send().await?.error_for_status()?.json().await?;
    for installation in [false, true] {
        let artifact = browser.artifact(&workspace, installation).await?;
        let archive = browser
            .request(reqwest::Method::GET, &artifact.download_path)
            .send()
            .await?;
        assert_eq!(archive.status(), StatusCode::OK);
        assert_eq!(
            archive
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/zip")
        );
        let bytes = archive.bytes().await?;
        assert!(
            bytes.starts_with(b"PK\x03\x04"),
            "the response is not a ZIP archive"
        );
        assert_eq!(bytes.len().to_string(), artifact.size);
        let denied = browser
            .http
            .get(format!("{}{}", browser.origin, artifact.download_path))
            .bearer_auth(&connector.secret)
            .send()
            .await?;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let error: okf_jawn_contract::error::ApiError = denied.json().await?;
        assert_eq!(error.code, ErrorCode::Forbidden);
    }
    running.stop().await?;
    Ok(())
}

#[tokio::test]
async fn the_binary_listens_and_a_second_writer_is_refused() -> TestResult {
    // Design section 1.8(10,11): stdout's order and health describe a real child, not compose().
    let installation = Installation::new()?;
    let settings = installation.local()?;
    let binary = installation.spawn_binary(&settings, &[], false)?;
    let first = binary.line()?;
    let origin = some(
        first.strip_prefix("okf-jawn-server: listening "),
        "the listening line",
    )?;
    assert!(binary.line()?.starts_with("okf-jawn-server: sandbox "));
    assert!(binary.line()?.starts_with("okf-jawn-server: launch "));
    alive_before_relay_deadline(origin, binary.started).await?;
    let mut second = installation.spawn_binary(&settings, &[], false)?;
    assert_eq!(second.exit()?.code(), Some(1));
    assert!(second.stderr()?.contains("owns the data directory"));
    assert_eq!(
        client()?
            .get(format!("{origin}/healthz"))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    Ok(())
}

#[test]
fn the_binary_refuses_configuration_and_usage_before_opening_data() -> TestResult {
    // Design section 1.8(12,13,15): each literal configuration/usage refusal has its own exit.
    for (mode, data, args, code, named) in [
        (Some("workos"), None, Vec::new(), 1, "WORKOS_CLIENT_ID"),
        (None, Some("relative"), Vec::new(), 1, "OKF_DATA_DIR"),
        (None, None, vec!["unknown-command"], 2, "unknown-command"),
    ] {
        let installation = Installation::new()?;
        let mut settings = installation.local()?;
        if let Some(mode) = mode {
            settings.insert("OKF_AUTH_MODE".to_owned(), mode.to_owned());
        }
        if let Some(data) = data {
            settings.insert("OKF_DATA_DIR".to_owned(), data.to_owned());
        }
        let mut binary = installation.spawn_binary(&settings, &args, false)?;
        assert_eq!(binary.exit()?.code(), Some(code));
        assert!(
            binary.stderr()?.contains(named),
            "the refusal must name {named}"
        );
        assert!(!installation.data().exists());
    }
    Ok(())
}

#[cfg(any(windows, unix))]
#[test]
fn the_binary_refuses_non_unicode_settings_instead_of_selecting_defaults() -> TestResult {
    // Design section 1.2: a present invalid setting is refused by name, never treated as unset.
    #[cfg(windows)]
    let invalid = {
        use std::os::windows::ffi::OsStringExt as _;
        OsString::from_wide(&[0xd800])
    };
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt as _;
        OsString::from_vec(vec![0xff])
    };
    let installation = Installation::new()?;
    let mut settings: BTreeMap<OsString, OsString> = installation
        .local()?
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
    // Keep an explicit valid data directory even on the failing implementation, so the red
    // cannot touch this user's per-machine default installation.
    settings.insert("OKF_AUTH_MODE".into(), invalid);
    let mut binary = installation.spawn_binary(&settings, &[], false)?;
    assert_eq!(binary.exit()?.code(), Some(1));
    assert!(binary.stderr()?.contains("OKF_AUTH_MODE"));
    assert!(!installation.data().exists());
    Ok(())
}

#[tokio::test]
async fn the_binary_keeps_serving_with_its_stdout_pipe_closed() -> TestResult {
    // Design section 1.8(14): a reader closes the actual stdout pipe; a null sink would not fail writes.
    let installation = Installation::new()?;
    let held = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = held.local_addr()?;
    drop(held);
    let mut settings = installation.local()?;
    settings.insert("OKF_BIND".to_owned(), address.to_string());
    settings.insert("OKF_SANDBOX_BIND".to_owned(), "127.0.0.1:0".to_owned());
    let binary = installation.spawn_binary(&settings, &[], true)?;
    alive_before_relay_deadline(&format!("http://{address}"), binary.started).await?;
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
