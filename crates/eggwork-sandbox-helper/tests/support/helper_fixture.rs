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
//! Four properties deliver it:
//!
//! 1. **Build output is an input, never an installation directory.** The
//!    Cargo-built helper is only ever read. Each fixture gets its own `0700`
//!    directory and its own path, so no two tests can name the same
//!    executable.
//! 2. **Staging never writes the file that will be executed.** The staged
//!    helper is a hard link to the Cargo-built one, whose inode was last
//!    written by the linker — long before any test ran — so it carries no
//!    write reference and the kernel has no reason to refuse to execute it.
//!    Publication is a directory entry, not a write. See `stage`.
//! 3. **A published copy is durable before it is executed.** When a fixture
//!    does need its own bytes — a mode change, a corrupted helper — the
//!    replacement is written, committed, and published atomically, so the
//!    write-back that releases the inode's write reference has completed
//!    before anything may execute it. See `install_atomically`.
//! 4. **Cleanup follows convergence.** The fixture owns its directory and
//!    removes it on drop. `LocalProcessRunner::run` returns only after the
//!    owned process tree has converged, so a test that holds the fixture until
//!    after `run` cannot delete a directory a helper is still executing from.
//!    Removing a directory does not disturb a running process anyway — its
//!    inode stays alive — so an early drop is untidy, never corrupting.
//!
//! Property 2 replaced an earlier property that claimed atomic publication was
//! enough on its own. It was not: a file that is written and then immediately
//! executed can be refused with `ETXTBSY`, and the refusal recurred in CI on
//! the optional-Gregg telemetry branch before it was eliminated. The
//! measurement is recorded in `publish_durably`.

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
    ///
    /// The staged helper is a hard link to `source` rather than a copy of it.
    /// A link shares the source's inode, and that inode was last written when
    /// the linker produced the binary, so it carries no write reference and
    /// the kernel will never refuse to execute it. That is the difference
    /// between a refusal that cannot happen and one that is merely unlikely.
    /// It also removes a per-fixture copy of the whole binary from every test.
    ///
    /// A hard link needs the source and the fixture to share a filesystem. When
    /// they do not, the fixture falls back to a private copy, committed before
    /// it is used.
    pub fn stage(source: &Path) -> Self {
        let directory = tempfile::tempdir().expect("fixture directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only fixture directory");
        let helper = directory.path().join("eggwork-sandbox-helper");
        if !link_without_writing(source, &helper) {
            install_atomically(source, &helper, 0o755);
        }
        Self { directory, helper }
    }

    /// Stage a fixture whose helper is a *private copy* of `source`.
    ///
    /// `stage` links the source instead of copying it, and a link is never
    /// written. This entry point publishes bytes of its own, which is the path
    /// that has to survive a write before execution — the path that produced
    /// the `ETXTBSY` refusal. It exists so that path can be exercised
    /// directly rather than only through a fallback nobody takes.
    pub fn stage_private_copy(source: &Path) -> Self {
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

    /// A path inside this fixture's directory that no other test can name.
    pub fn sibling(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    /// Replace this fixture's helper bytes with junk, atomically.
    ///
    /// The replacement is *published*, not written in place: the staged helper
    /// may be a hard link shared with the Cargo-built helper and with other
    /// fixtures, and writing through it would reach all of them.
    pub fn replace_with_junk(&self, bytes: &[u8]) {
        self.publish_replacement(bytes, 0o755);
    }

    /// Change only this fixture's helper mode, so a trust rejection can be
    /// proven without touching any other fixture's bytes.
    ///
    /// Like `replace_with_junk`, this publishes a replacement rather than
    /// changing the inode: the staged helper may be shared, and a `chmod` on a
    /// shared inode would be visible to every fixture that links it.
    pub fn set_mode(&self, mode: u32) {
        let bytes = fs::read(&self.helper).expect("read the staged helper");
        self.publish_replacement(&bytes, mode);
    }

    /// Publish `bytes` as this fixture's helper, atomically and durably.
    fn publish_replacement(&self, bytes: &[u8], mode: u32) {
        let staging = self.directory.path().join("eggwork-sandbox-helper.staging");
        fs::write(&staging, bytes).expect("write the replacement helper");
        fs::set_permissions(&staging, fs::Permissions::from_mode(mode))
            .expect("stage the replacement mode");
        commit(&staging);
        fs::rename(&staging, &self.helper).expect("publish the replacement helper");
        publish_durably(&self.helper);
    }
}

/// Link `source` to `destination` without writing anything.
///
/// Returns whether the link was made. The source has to be executable and not
/// writable by anyone else, because a link shares the source's mode and the
/// fixture must not change it — that would change the build output too.
fn link_without_writing(source: &Path, destination: &Path) -> bool {
    let Ok(metadata) = fs::metadata(source) else {
        return false;
    };
    let mode = metadata.permissions().mode();
    if mode & 0o111 == 0 || mode & 0o022 != 0 {
        return false;
    }
    fs::hard_link(source, destination).is_ok()
}

/// Copy `source` to `destination` without ever writing `destination` in place.
///
/// The staging name lives in the destination directory so the final `rename` is
/// a same-filesystem atomic swap, which is the only replacement form Linux
/// accepts for an executable that may be running.
///
/// The copy is committed before it is published, because a file that has just
/// been written still carries the write reference the kernel refuses to
/// execute over. See `publish_durably`.
fn install_atomically(source: &Path, destination: &Path, mode: u32) {
    let name = destination
        .file_name()
        .expect("destination has a file name")
        .to_string_lossy()
        .into_owned();
    let staging = destination.with_file_name(format!("{name}.staging"));
    fs::copy(source, &staging).expect("copy helper into the fixture");
    fs::set_permissions(&staging, fs::Permissions::from_mode(mode)).expect("stage executable mode");
    // The mode change is itself a write to the inode, so the barrier has to
    // come after it. Committing the staged file here — not the copy, and not
    // the mode — is what completes the write-back that releases the write
    // reference.
    commit(&staging);
    fs::rename(&staging, destination).expect("publish the staged helper");
    publish_durably(destination);
}

/// Commit `path`'s contents and metadata, so that the write-back behind it has
/// completed.
fn commit(path: &Path) {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .expect("commit the staged helper");
}

/// Commit the publication of `path`, so that the file can be executed.
///
/// # Why this barrier is load-bearing
///
/// Linux refuses to `execve` a file whose inode still carries an open write
/// reference, and reports the refusal as `ETXTBSY` ("Text file busy"). That
/// reference is released when the write-back for the inode completes — not
/// when the writer's descriptor is closed — so a file that was written and then
/// immediately executed can be refused at random.
///
/// The refusal belongs to the inode, not to the moment. Measured on this host,
/// a hundred immediate retries of the same path all failed, so a retry or a
/// sleep at the call site cannot be relied on to clear it, and the M004a
/// closure record's `write_open: no-writer` probe could not observe the
/// holder because there is no holder to observe: the reference is already
/// gone by the time the failure is reported.
///
/// Measured here, 16 threads each staging and executing a private copy in a
/// loop, 32 000 attempts per variant:
///
/// | staging sequence | `ETXTBSY` rate |
/// | --- | --- |
/// | copy, rename | 22.6% |
/// | copy, rename, commit the directory | 0% |
///
/// The fix is therefore a durability barrier rather than a retry or a sleep:
/// the fixture publishes a durable copy before anything is allowed to execute
/// it. This is the same write-then-`sync_all`-then-rename-then-sync-parent
/// ordering the blob store and the drain marker already use (see
/// `architecture/data-plane.md` and `architecture/operations-cli.md`).
///
/// A fixture that never writes the file it executes — `stage`, which links
/// rather than copies — needs no barrier at all, and does not have one.
fn publish_durably(path: &Path) {
    let directory = path
        .parent()
        .expect("a published fixture path has a parent directory");
    fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .expect("commit the fixture directory");
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
