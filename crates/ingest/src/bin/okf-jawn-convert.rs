//! The conversion child: converts one page window of one document under the memory cap, and
//! writes its result into the output directory the supervisor gave it.
//!
//! The integration owner declares this target with `required-features = ["runtime"]` (I4). The
//! body is gated on the same feature, so a build without it still compiles.
//!
//! Limitation, stated rather than hidden: the conversion itself is not built yet. Until it is,
//! this child refuses every request with a non-zero exit and writes no result. The supervisor
//! reads that as a crash (`cap::classify`), never as a converted window.

use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let _written = writeln!(
        std::io::stderr(),
        "okf-jawn-convert: the conversion child is not built yet; no window was converted"
    );
    ExitCode::FAILURE
}
