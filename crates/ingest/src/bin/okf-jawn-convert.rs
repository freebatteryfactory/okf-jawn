//! The conversion child: converts one page window of one document under the memory cap, and
//! writes its result into the output directory the supervisor gave it (`ingest::child`).
//!
//! The integration owner declares this target with `required-features = ["runtime"]` (I4). The
//! body is gated on the same feature, so a build without it still compiles; that build refuses
//! every request with a failing exit and no result, which the supervisor reads as a crash.

use std::process::ExitCode;

#[cfg(feature = "runtime")]
fn main() -> ExitCode {
    // The cap comes first, before the request is read or a model is loaded (Linux sets
    // RLIMIT_DATA here; elsewhere the supervisor holds it).
    if let Err(error) = okf_jawn_ingest::cap::limit_self() {
        let _written = std::io::Write::write_fmt(
            &mut std::io::stderr(),
            format_args!("okf-jawn-convert: {}\n", error.message),
        );
        return ExitCode::FAILURE;
    }
    okf_jawn_ingest::child::main()
}

#[cfg(not(feature = "runtime"))]
fn main() -> ExitCode {
    let _written = std::io::Write::write_fmt(
        &mut std::io::stderr(),
        format_args!("okf-jawn-convert: built without the runtime feature; nothing converted\n"),
    );
    ExitCode::FAILURE
}
