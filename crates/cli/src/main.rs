//! The `okf-jawn` executable: the process's arguments, environment and standard streams handed
//! to `okf_jawn_cli::run`.
//!
//! The runtime is shut down without waiting for its blocking threads, so a read of standard
//! input that is still pending never holds the process open after the run has ended.

use std::io::Write;
use std::process::ExitCode;

use okf_jawn_cli::{Streams, run};

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "okf-jawn: internal: the runtime could not be started: {error}"
            );
            return ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(run(
        std::env::args_os(),
        &|name| std::env::var(name).ok(),
        Streams {
            stdin: tokio::io::stdin(),
            stdout: tokio::io::stdout(),
            stderr: tokio::io::stderr(),
        },
    ));
    runtime.shutdown_background();
    ExitCode::from(code)
}
