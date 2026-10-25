//! Capability advertisement is backed by the same trust check the runner uses at
//! execution time, so this test has to present the helper the way an
//! installation does: owner-only directory, executable file.
//!
//! The staging rules live in one place, shared with the Landlock suite, because
//! two copies of "copy the built helper somewhere trustable" is exactly how a
//! fixture starts overwriting an executable another test may be running
//! (Operations M004a). The `#[path]` include is deliberate: the alternative is a
//! dev-only workspace member, and the release contract enumerates workspace
//! members.

use eggwork_runner::{LANDLOCK_WORKSPACE_RW_CAPABILITY, LocalProcessRunner, TrustedLandlockSetup};
#[cfg(target_os = "linux")]
use std::path::PathBuf;

// One shared fixture surface, two suites: this one only needs `stage` and
// `path`, so the rest would be dead code here and in use next door.
#[allow(dead_code)]
#[cfg(target_os = "linux")]
#[path = "../../eggwork-sandbox-helper/tests/support/helper_fixture.rs"]
mod helper_fixture;

/// The built sandbox helper, located deterministically.
///
/// The helper belongs to another package, so `CARGO_BIN_EXE_eggwork-sandbox-helper`
/// is not available here: Cargo only sets that variable for the package that
/// owns the binary. What *is* available is this test's own executable, which
/// Cargo places under the target directory's `deps` directory, one level below
/// the profile directory that holds the package's binaries. The helper is
/// therefore the sibling of the `deps` directory — the same walk
/// `eggwork_server`'s `locate_helper_binary` performs.
///
/// The lookup fails **loudly**. A test that returns early when it cannot find
/// its fixture reports success without asserting anything, which is a hole
/// rather than a check: the previous implementation did exactly that, and the
/// skip was the normal path in CI as well as locally.
#[cfg(target_os = "linux")]
fn built_helper() -> PathBuf {
    let executable = std::env::current_exe().expect("the test executable has a path");
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(directory) = executable.parent() {
        candidates.push(directory.join("eggwork-sandbox-helper"));
        if let Some(profile) = directory.parent() {
            candidates.push(profile.join("eggwork-sandbox-helper"));
        }
    }
    if let Some(found) = candidates.iter().find(|candidate| candidate.is_file()) {
        return found.clone();
    }
    panic!(
        "eggwork-sandbox-helper was not found next to this test executable.\n\
         expected one of: {}\n\
         build it first: cargo build --locked -p eggwork-sandbox-helper",
        candidates
            .iter()
            .map(|candidate| candidate.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn trusted_helper_advertises_landlock_workspace_rw_capability() {
    let source = built_helper();
    let fixture = helper_fixture::TrustedHelperFixture::stage(&source);
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(fixture.path()));
    let capabilities = runner.execution_capabilities().await;
    assert!(
        capabilities
            .iter()
            .any(|feature| feature == LANDLOCK_WORKSPACE_RW_CAPABILITY),
        "expected Landlock workspace-rw capability on a kernel with ABI V4; got {capabilities:?}"
    );
}

/// Platform-neutral negative case: a helper that is not there must not produce
/// a capability. On Windows there is no Landlock at all, so the assertion holds
/// there for a stronger reason than on Linux.
#[tokio::test]
async fn missing_helper_advertises_no_landlock_capability() {
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(
        "/nonexistent/eggwork-sandbox-helper",
    ));
    let capabilities = runner.execution_capabilities().await;
    assert!(
        !capabilities
            .iter()
            .any(|feature| feature == LANDLOCK_WORKSPACE_RW_CAPABILITY),
        "missing helper must not advertise Landlock capability; got {capabilities:?}"
    );
}
