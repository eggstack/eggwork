//! One reusable test-fixture helper for staging a *trusted* sandbox helper.
//!
//! This module is included by both crates whose tests need an installed,
//! trust-passing helper — `eggwork-sandbox-helper`'s Landlock suite and
//! `eggwork-runner`'s capability probe — so the staging behaviour cannot drift
//! between them. It is test-only code and never enters a shipped binary.
//!
//! # Why this exists
//!
//! Operations M004 recorded one same-head CI failure of
//! `required_landlock_allows_workspace_and_denies_outside_reads_and_writes`
//! with `Spawn("Text file busy (os error 26)")`, which passed on an immediate
//! rerun. `ETXTBSY` is what Linux returns when a file that is currently being
//! executed is opened for writing, and it is also what a partially written
//! executable can be replaced with mid-flight. The invariant this module exists
//! to make true is:
//!
//! > A fixture must never mutate, truncate, overwrite, or rename over an
//! > executable path another concurrently runnable test may execute.
//!
//! Three properties deliver it:
//!
//! 1. **Build output is an input, never an installation directory.** The
//!    Cargo-built helper is only ever read. Each fixture gets its own
//!    `0700` directory and its own copy, so no two tests can name the same
//!    executable.
//! 2. **Staging is atomic.** The copy is written under a staging name and
//!    `rename`d into place, never written in place. Rename over a running
//!    executable is allowed on Linux; `open(O_TRUNC)` is not. A fixture
//!    therefore cannot hit `ETXTBSY`, and no execution can observe a
//!    half-written helper.
//! 3. **Cleanup follows convergence.** The fixture owns its directory and
//!    removes it on drop. `LocalProcessRunner::run` returns only after the
//!    owned process tree has converged, so a test that holds the fixture until
//!    after `run` cannot delete a directory a helper is still executing from.
//!    Removing a directory does not disturb a running process anyway — its
//!    inode stays alive — so an early drop is untidy, never corrupting.

// Every include site gates this module with `#[cfg(target_os = "linux")]`; the
// gate lives there rather than here so including this file does not emit a
// duplicated-attribute warning.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A fixture-owned installation of the sandbox helper.
///
/// Dropping it removes its directory. It is `Send` so a test can move it into
/// a task when proving that two fixtures execute concurrently.
pub struct TrustedHelperFixture {
    // Owned for its `Drop`, not just to be read: some suites only ever ask for
    // `path()`, and the directory still has to be removed when they finish.
    #[allow(dead_code)]
    directory: tempfile::TempDir,
    helper: PathBuf,
}

impl TrustedHelperFixture {
    /// Stage `source` — normally the Cargo-built helper — into a fresh private
    /// installation.
    ///
    /// The directory is `0700` and the copy `0755`, which is exactly what
    /// `verify_trusted_helper` requires: an owner-only ancestor chain, a
    /// regular file, and an executable mode. Trust is therefore satisfied by
    /// construction, which is part of the contract under test rather than an
    /// accident.
    pub fn stage(source: &Path) -> Self {
        let directory = tempfile::tempdir().expect("fixture directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only fixture directory");
        let helper = directory.path().join("eggwork-sandbox-helper");
        install_atomically(source, &helper, 0o755);
        Self { directory, helper }
    }

    /// The trusted path to hand to `TrustedLandlockSetup`.
    pub fn path(&self) -> &Path {
        &self.helper
    }

    /// The fixture's private installation directory.
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    /// Replace this fixture's helper bytes with junk, atomically.
    ///
    /// This exists for the negative trust cases. It deliberately uses the same
    /// atomic path as staging, so a test can prove a *neighbouring* fixture is
    /// unaffected without itself creating the race it is testing against.
    pub fn replace_with_junk(&self, bytes: &[u8]) {
        let staging = self.directory.path().join("eggwork-sandbox-helper.staging");
        fs::write(&staging, bytes).expect("write junk helper");
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o755)).expect("junk mode");
        fs::rename(&staging, &self.helper).expect("replace helper atomically");
    }

    /// Change only this fixture's helper mode, so a trust rejection can be
    /// proven without touching any other fixture's bytes.
    pub fn set_mode(&self, mode: u32) {
        fs::set_permissions(&self.helper, fs::Permissions::from_mode(mode))
            .expect("set helper mode");
    }

    /// A path inside this fixture's directory that no other test can name.
    pub fn sibling(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }
}

/// Copy `source` to `destination` without ever writing `destination` in place.
///
/// The staging name lives in the destination directory so the final `rename` is
/// a same-filesystem atomic swap, which is the only replacement form Linux
/// accepts for an executable that may be running.
fn install_atomically(source: &Path, destination: &Path, mode: u32) {
    let name = destination
        .file_name()
        .expect("destination has a file name")
        .to_string_lossy()
        .into_owned();
    let staging = destination.with_file_name(format!("{name}.staging"));
    fs::copy(source, &staging).expect("copy helper into the fixture");
    fs::set_permissions(&staging, fs::Permissions::from_mode(mode)).expect("stage executable mode");
    fs::rename(&staging, destination).expect("publish the staged helper");
}

/// The digest of a file, used by the fixture tests to prove that staging never
/// mutates its source and that one fixture cannot corrupt another.
///
/// This is the product's own version-prefixed digest rather than a local
/// implementation, so the fixture tests assert against the same definition the
/// release path uses.
pub fn digest(path: &Path) -> String {
    eggwork_core::BlobDigest::from_bytes(&fs::read(path).expect("read helper bytes"))
        .as_str()
        .to_owned()
}
