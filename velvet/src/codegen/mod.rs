mod codegen;
pub use codegen::generate;
mod func_finding;
use func_finding::*;
mod quotes;
use quotes::*;

use std::sync::atomic::{AtomicBool, Ordering};

// whether an error has been reported (the generated code is then not written)
static ERRORS_REPORTED: AtomicBool = AtomicBool::new(false);

/*
    Reports an error to cargo (`cargo::error=`, one directive per line). Cargo shows each line as an error and fails
    the build once the build script has finished, so all errors can be reported before stopping, instead of
    exiting at the first one.
*/
fn report_error(msg: &str) {
    ERRORS_REPORTED.store(true, Ordering::Relaxed);
    for line in msg.lines() {
        println!("cargo::error={}", line);
    }
}

fn errors_reported() -> bool {
    ERRORS_REPORTED.load(Ordering::Relaxed)
}
