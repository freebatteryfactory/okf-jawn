//! Ruling 4, start or connect: connect to a running service, start the local server only when
//! a loopback address refuses connections, start it once, never for a remote address, and
//! wait a bounded time for it.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use okf_jawn_cli::Endpoint;
use okf_jawn_cli::relay::{
    Arrival, ChildState, Clock, Detachment, Launcher, Probe, ProcessLauncher, StartPolicy,
    server_beside, start_or_connect,
};
use serde_json::json;

use super::request;
use crate::check::{TestResult, err_of, some};
use crate::support::doubles::{
    FakeClock, FakeLauncher, FakeTransport, Reports, Setup, accepted, relay, relay_over,
};
use crate::support::{EXECUTABLE, Untouched};

/// What one run of `start_or_connect` on a clock the test moves came to.
struct Waited {
    arrival: Result<Arrival, String>,
    /// How long the clock ran after the launch.
    took: Duration,
    /// What the relay said meanwhile, with the seconds since the launch.
    said: Vec<(u64, String)>,
    launched: usize,
    probed: usize,
}

/// How long, in real time, a test lets a wait run that a correct relay ends at once.
const PATIENCE: Duration = Duration::from_secs(20);

/// A transport whose liveness checks answer `probes` in order and then `otherwise`.
fn probing(probes: Vec<Probe>, otherwise: Probe) -> Arc<FakeTransport> {
    FakeTransport::probing(
        |_, _| Box::pin(async { accepted().reply }),
        probes,
        otherwise,
    )
}

/// Run `start_or_connect` with the default policy against checks that answer `probes` and
/// then `otherwise`. The check before the launch costs nothing; each one after it costs
/// `cost` on the clock.
async fn wait(probes: Vec<Probe>, otherwise: Probe, cost: Duration, state: ChildState) -> Waited {
    let clock = FakeClock::default();
    let transport = FakeTransport::costing(probes, otherwise, &clock, vec![Duration::ZERO], cost);
    let mut launcher = FakeLauncher {
        state,
        ..FakeLauncher::default()
    };
    let mut reports = Reports::on(&clock);
    // The clock is the test's own, so a correct wait takes no real time; a wait that never
    // ends is a failure of the test, not a test that never ends.
    let arrival = tokio::time::timeout(
        PATIENCE,
        start_or_connect(
            transport.as_ref(),
            &mut launcher,
            true,
            StartPolicy::default(),
            &clock,
            &mut reports,
        ),
    )
    .await
    .unwrap_or_else(|_| Err("the wait did not end in real time".to_owned()));
    Waited {
        arrival,
        took: clock.now(),
        said: reports
            .said
            .into_iter()
            .map(|(at, text)| (at.as_secs(), text))
            .collect(),
        launched: launcher.launched(),
        probed: transport.probed(),
    }
}

/// Run `start_or_connect` once on a clock nothing costs anything on.
async fn arrive(
    transport: &FakeTransport,
    launcher: &mut FakeLauncher,
    loopback: bool,
) -> Result<Arrival, String> {
    let clock = FakeClock::default();
    start_or_connect(
        transport,
        launcher,
        loopback,
        StartPolicy::default(),
        &clock,
        &mut Reports::on(&clock),
    )
    .await
}

#[tokio::test]
async fn a_running_service_is_connected_to_and_nothing_is_started() -> TestResult {
    let transport = probing(Vec::new(), Probe::Up);
    let mut launcher = FakeLauncher::default();
    let arrival = arrive(transport.as_ref(), &mut launcher, true).await?;
    assert_eq!(arrival, Arrival::Connected);
    assert_eq!((launcher.launched(), transport.probed()), (0, 1));
    Ok(())
}

/// A loopback address that refuses connections: the server is launched once, and the relay
/// keeps asking through every kind of "not yet" until the service answers.
#[tokio::test]
async fn a_refusing_loopback_address_starts_the_server_once_and_waits_for_it() -> TestResult {
    let transport = probing(
        vec![
            Probe::Refused,
            Probe::Refused,
            Probe::Failed("the check timed out".to_owned()),
            Probe::Unhealthy(503),
        ],
        Probe::Up,
    );
    let mut launcher = FakeLauncher::default();
    let arrival = arrive(transport.as_ref(), &mut launcher, true).await?;
    assert_eq!(arrival, Arrival::Started);
    assert_eq!((launcher.launched(), transport.probed()), (1, 5));
    Ok(())
}

#[tokio::test]
async fn a_remote_address_is_never_started() -> TestResult {
    let transport = probing(Vec::new(), Probe::Refused);
    let mut launcher = FakeLauncher::default();
    let why = err_of(arrive(transport.as_ref(), &mut launcher, false).await)?;
    assert!(why.contains("never starts a remote service"), "{why}");
    assert_eq!((launcher.launched(), transport.probed()), (0, 1));
    Ok(())
}

/// Something answers at the address, or the check fails otherwise: nothing is started.
#[tokio::test]
async fn an_address_that_answers_or_fails_otherwise_starts_nothing() -> TestResult {
    for (probe, said) in [
        (Probe::Unhealthy(404), "HTTP 404"),
        (Probe::Unhealthy(503), "HTTP 503"),
        (
            Probe::Failed("the check timed out".to_owned()),
            "the check timed out",
        ),
    ] {
        for loopback in [true, false] {
            let transport = probing(vec![probe.clone()], Probe::Up);
            let mut launcher = FakeLauncher::default();
            let why = err_of(arrive(transport.as_ref(), &mut launcher, loopback).await)?;
            assert!(why.contains(said), "{why}");
            assert_eq!(
                (launcher.launched(), transport.probed()),
                (0, 1),
                "{probe:?}"
            );
        }
    }
    Ok(())
}

/// Two relays start at once and this one's server loses the data directory to the other's:
/// its process has ended, yet it keeps waiting, connects to the winner and launches nothing
/// more.
#[tokio::test]
async fn after_its_server_ends_the_relay_keeps_waiting_and_never_starts_another() -> TestResult {
    let waited = wait(
        vec![Probe::Refused; 4],
        Probe::Up,
        Duration::from_secs(1),
        ChildState::Exited(Some(1)),
    )
    .await;
    assert_eq!(waited.arrival, Ok(Arrival::Started));
    assert_eq!((waited.launched, waited.probed), (1, 5));
    Ok(())
}

/// The wait is bounded by the clock, whatever each check costs, and from both sides. Each
/// check after the launch costs 7 s and follows a pause of half a second, so the sixth ends
/// exactly 45 s after the launch: an answer there is taken. A seventh check would end at
/// 52.5 s: it is never started, and the relay gives up at 45 s by the clock.
#[tokio::test]
async fn the_wait_for_a_started_server_is_bounded_by_the_clock() -> TestResult {
    let seven = Duration::from_secs(7);
    let in_time = wait(
        vec![Probe::Refused; 6],
        Probe::Up,
        seven,
        ChildState::Running,
    )
    .await;
    assert_eq!(in_time.arrival, Ok(Arrival::Started));
    assert_eq!(
        (in_time.took, in_time.launched, in_time.probed),
        (Duration::from_secs(45), 1, 7)
    );

    for (state, said) in [
        (ChildState::Running, "left running"),
        (ChildState::Exited(Some(3)), "ended with exit code 3"),
        (ChildState::Exited(None), "ended and no service answered"),
        (
            ChildState::Unknown("no such process".to_owned()),
            "no such process",
        ),
    ] {
        let too_late = wait(vec![Probe::Refused; 7], Probe::Up, seven, state).await;
        let why = err_of(too_late.arrival)?;
        assert!(
            why.contains("within 45 seconds") && why.contains(said),
            "{why}"
        );
        assert_eq!(
            (too_late.took, too_late.launched, too_late.probed),
            (Duration::from_secs(45), 1, 7)
        );
    }

    // A check that costs more than what is left is given up when the time is up: the sixth
    // one after the launch takes 8 s, is started at 43 s, and would have answered at 51 s.
    let cut_off = wait(
        vec![Probe::Refused; 6],
        Probe::Up,
        Duration::from_secs(8),
        ChildState::Running,
    )
    .await;
    err_of(cut_off.arrival)?;
    assert_eq!((cut_off.took, cut_off.probed), (Duration::from_secs(45), 7));

    // A check that never answers does not hold the relay either.
    let hanging = wait(
        vec![Probe::Refused],
        Probe::Up,
        Duration::from_secs(1_000_000),
        ChildState::Running,
    )
    .await;
    err_of(hanging.arrival)?;
    assert_eq!((hanging.took, hanging.probed), (Duration::from_secs(45), 2));
    Ok(())
}

/// While it waits the relay says what it is doing: at the launch, and then every ten seconds
/// by the clock, whether the checks are quick, slow or never answer. It says nothing when it
/// only connects.
#[tokio::test]
async fn a_relay_that_waits_for_its_server_says_so_at_least_every_ten_seconds() -> TestResult {
    for cost in [
        Duration::ZERO,
        Duration::from_secs(7),
        Duration::from_secs(1_000_000),
    ] {
        let waited = wait(Vec::new(), Probe::Refused, cost, ChildState::Running).await;
        err_of(waited.arrival)?;
        let times: Vec<u64> = waited.said.iter().map(|(at, _)| *at).collect();
        assert_eq!(times, [0, 10, 20, 30, 40], "{cost:?}: {:?}", waited.said);
        let first = some(waited.said.first(), "the first word")?;
        assert!(
            first.1.contains("started the okf-jawn server") && first.1.contains("up to 45 seconds"),
            "{}",
            first.1
        );
        let last = some(waited.said.last(), "the last word")?;
        assert_eq!(
            last.1,
            "still waiting for the okf-jawn server to answer (40 of 45 seconds)"
        );
    }
    let connected = wait(
        Vec::new(),
        Probe::Up,
        Duration::ZERO,
        ChildState::NotStarted,
    )
    .await;
    assert_eq!(
        (connected.arrival, connected.said, connected.launched),
        (Ok(Arrival::Connected), Vec::new(), 0)
    );
    Ok(())
}

/// The default wait is the stated one: 45 seconds in all, a check every half second, and a
/// word at least every ten seconds.
#[test]
fn mcp_stdio_default_start_wait_is_the_stated_one() {
    assert_eq!(
        StartPolicy::default(),
        StartPolicy {
            wait: Duration::from_secs(45),
            pause: Duration::from_millis(500),
            report_every: Duration::from_secs(10),
        }
    );
}

/// The service is not running and cannot be started: stderr says why, the exit code is 1,
/// stdin is not read and stdout stays empty.
#[tokio::test]
async fn a_service_that_cannot_be_started_is_explained_and_the_relay_exits_1() -> TestResult {
    let transport = probing(Vec::new(), Probe::Refused);
    let mut setup = Setup::new(&transport);
    setup.launcher.outcome = Err("okf-jawn-server could not be started: not found".to_owned());
    let launcher = setup.launcher.clone();
    let read = Arc::new(AtomicBool::new(false));
    let mut stdout = Vec::new();
    let (code, stderr) = relay_over(setup, Untouched(read.clone()), &mut stdout).await?;
    assert_eq!((code, stdout.len()), (1, 0));
    assert!(!read.load(Ordering::SeqCst), "stdin was read");
    assert!(
        stderr.starts_with("okf-jawn mcp: No service is running, and the server could not be started: okf-jawn-server could not be started: not found."),
        "{stderr}"
    );
    assert!(
        stderr.contains("--server-bin") && stderr.contains("OKF_JAWN_SERVER_BIN"),
        "{stderr}"
    );
    assert_eq!((launcher.launched(), transport.probed()), (1, 1));
    assert_eq!(transport.posts()?, Vec::new(), "nothing was forwarded");
    Ok(())
}

/// A relay that started the server says so on stderr, when it starts it and when it answers,
/// then relays as usual.
#[tokio::test]
async fn a_started_service_is_announced_on_stderr_and_then_used() -> TestResult {
    let transport = FakeTransport::probing(
        |_, _| Box::pin(async { accepted().reply }),
        vec![Probe::Refused],
        Probe::Up,
    );
    let setup = Setup::new(&transport);
    let launcher = setup.launcher.clone();
    let ran = relay(setup, request(&json!(1), "ping", &json!({})).as_bytes()).await?;
    assert_eq!(ran.code, 0);
    assert_eq!(launcher.launched(), 1);
    assert_eq!(
        ran.stderr,
        "okf-jawn mcp: no service answers at the server address; started the okf-jawn server \
         and waiting up to 45 seconds for it to answer\n\
         okf-jawn mcp: the okf-jawn server the relay started answers; it is left running when \
         the relay ends\n"
    );
    assert_eq!(transport.posts()?.len(), 1);
    Ok(())
}

/// The default server executable is the one shipped beside the CLI, with the platform's
/// suffix, and the launcher gives it the address the relay will connect to.
#[test]
fn mcp_stdio_server_beside_the_cli_is_started_on_the_server_address() -> TestResult {
    let beside = server_beside(Path::new("/opt/okf-jawn/bin/okf-jawn"));
    assert_eq!(
        beside,
        Path::new("/opt/okf-jawn/bin")
            .join(format!("okf-jawn-server{}", std::env::consts::EXE_SUFFIX))
    );
    for (address, bind) in [
        ("http://127.0.0.1:7711", "127.0.0.1:7711"),
        ("http://localhost:8123", "127.0.0.1:8123"),
        ("http://[::1]:9000", "[::1]:9000"),
        ("http://127.0.0.1", "127.0.0.1:80"),
        ("https://localhost", "127.0.0.1:443"),
    ] {
        assert_eq!(
            Endpoint::parse(address)?.bind_address().as_deref(),
            Some(bind),
            "{address}"
        );
    }
    let launcher = ProcessLauncher::new(Some(beside.clone()), Some("127.0.0.1:7711".to_owned()));
    let (command, detachment) = some(launcher.command(), "a command")?;
    // The started process is cut loose from the terminal the relay runs in: on Windows a
    // console of its own without a window and its own process group, on Unix its own
    // process group, which is also what the platform's `Command` shows it was told.
    #[cfg(windows)]
    assert_eq!(
        detachment,
        Detachment::CreationFlags(0x0800_0000 | 0x0000_0200)
    );
    #[cfg(unix)]
    {
        assert_eq!(detachment, Detachment::ProcessGroup(0));
        let told: String = format!("{command:#?}").split_whitespace().collect();
        assert!(told.contains("pgroup:Some(0"), "{told}");
    }
    assert_eq!(command.get_program(), beside.as_os_str());
    assert_eq!(command.get_args().count(), 0);
    let bind: Vec<_> = command
        .get_envs()
        .filter(|(name, _)| *name == "OKF_BIND")
        .map(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(bind, [Some("127.0.0.1:7711".to_owned())]);
    // The connector credential is the relay's own: the server it starts, and the conversion
    // child that server starts, never inherit it.
    let token: Vec<bool> = command
        .get_envs()
        .filter(|(name, _)| *name == "OKF_JAWN_TOKEN")
        .map(|(_, value)| value.is_none())
        .collect();
    assert_eq!(token, [true], "OKF_JAWN_TOKEN is not removed");
    Ok(())
}

/// The production launcher against real processes: an executable that is not there is
/// reported by its path, and one that ends is reported as ended with its exit code.
#[test]
fn mcp_stdio_production_launcher_reports_what_became_of_the_process() -> TestResult {
    let mut unknown = ProcessLauncher::new(None, None);
    assert_eq!(unknown.state(), ChildState::NotStarted);
    assert!(err_of(unknown.launch())?.contains("not known"));

    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-okf-jawn-server");
    let mut absent = ProcessLauncher::new(Some(missing.clone()), None);
    let why = err_of(absent.launch())?;
    assert!(why.contains(&missing.display().to_string()), "{why}");
    assert_eq!(absent.state(), ChildState::NotStarted);

    // The CLI executable stands in for the server: without arguments it ends at once with
    // the usage exit code, whatever the bind address it is given.
    let stand_in = Path::new(EXECUTABLE).to_path_buf();
    let mut ending = ProcessLauncher::new(Some(stand_in), Some("127.0.0.1:1".to_owned()));
    ending.launch()?;
    let mut state = ending.state();
    for _ in 0..600 {
        if state != ChildState::Running {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        state = ending.state();
    }
    assert_eq!(state, ChildState::Exited(Some(2)));
    Ok(())
}

/// The relay is a client and nothing more: the CLI crate depends on none of the crates that
/// hold or serve a store, so it cannot open one.
#[test]
fn mcp_stdio_cli_depends_on_no_crate_that_holds_a_store() -> TestResult {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))?;
    assert!(manifest.contains("okf-jawn-contract"), "{manifest}");
    for name in ["server", "mcp", "core", "storage", "ingest"] {
        assert!(
            !manifest.contains(&format!("okf-jawn-{name}")),
            "the CLI manifest names okf-jawn-{name}"
        );
    }
    Ok(())
}
