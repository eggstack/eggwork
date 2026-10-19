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
//! rerun. `ETXTBSY` is what Linux returns when a file that is currently open for
//! writing is executed. The invariant this module exists to make true is:
//!
//! > A fixture must never mutate, truncate, overwrite, or rename over an
//! > executable path another concurrently runnable test may execute.
//!
//! # What the kernel actually does
//!
//! The refusal is not a timing window and it is not about write-back. Linux
//! accounts write access on the inode itself: `struct inode::i_writecount` is
//! positive while the file is open for writing, negative while it is being
//! executed, and zero otherwise. `execve()` opens the candidate executable and
//! then calls `exe_file_deny_write_access()`, which is `deny_write_access()`:
//!
//! ```c
//! static inline int deny_write_access(struct file *file)
//! {
//!     struct inode *inode = file_inode(file);
//!     return atomic_dec_unless_positive(&inode->i_writecount) ? 0 : -ETXTBSY;
//! }
//! ```
//!
//! The write access itself is taken by every write-open of a regular file, in
//! `do_dentry_open()`, and released when that file is torn down. So an `execve()`
//! of an inode that is *still* open for writing fails with `ETXTBSY`, and the
//! release is asynchronous with respect to the closing thread — which is why the
//! refusal can be observed on a file that no `/proc/<pid>/fd` entry points at.
//!
//! Three consequences follow, and they are the whole design:
//!
//! 1. `fsync`, `O_DSYNC` and sleeping do not help. They concern write-back;
//!    the refusal is caused by the write *access*.
//! 2. A retry does not help either, because a refused `execve` leaves the
//!    access in place.
//! 3. The only construction that cannot produce the refusal is one that never
//!    opens the executed inode for writing.
//!
//! # The four properties
//!
//! 1. **Build output is an input, never an installation directory.** The
//!    Cargo-built helper is only ever read. Each fixture gets its own `0700`
//!    directory and its own path, so no two tests can name the same
//!    executable.
//! 2. **Staging never writes the file that will be executed.** The staged
//!    helper is a hard link to the Cargo-built one. A link shares the source's
//!    inode and does not open it for writing, so the inode's write access is
//!    never taken and the kernel has no reason to refuse to execute it. See
//!    `stage`.
//! 3. **A link shares the source's mode, so the source has to be trusted.**
//!    `verify_trusted_helper` rejects a helper that anyone but its owner may
//!    write, and Cargo's executable mode is `0666 & ~umask` plus the execute
//!    bits — so a permissive umask produces a group-writable build output that
//!    no fixture could stage by linking. The fixture therefore clears the group
//!    and world write bits of the build output once, before anything is staged.
//!    That is a mode change on a shared build artifact, and it is deliberately
//!    one-directional: it can only ever *remove* write permission for others,
//!    and the build output's bytes are never touched. See
//!    `normalize_trusted_mode`.
//! 4. **Cleanup follows convergence.** The fixture owns its directory and
//!    removes it on drop. `LocalProcessRunner::run` returns only after the
//!    owned process tree has converged, so a test that holds the fixture until
//!    after `run` cannot delete a directory a helper is still executing from.
//!    Removing a directory does not disturb a running process anyway — its
//!    inode stays alive — so an early drop is untidy, never corrupting.
//!
//! Property 2 replaced an earlier property that claimed atomic publication was
//! enough on its own. It was not: a file that is written and then immediately
//! executed can be refused with `ETXTBSY`, and the refusal recurred in CI on the
//! optional-Gregg telemetry branch before it was eliminated. The measurement is
//! recorded in `plans/closure/operations-distribution/004b-status.md`.
//!
//! Property 3 is new with the same corrective. It is the price of property 2:
//! a link cannot have a different mode from the file it links to.

// Every include site gates this module with `#[cfg(target_os = "linux")]`; the
// gate lives there rather than here so including this file does not emit a
// duplicated-attribute warning.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The file name a staged helper is published under.
const HELPER_NAME: &str = "eggwork-sandbox-helper";

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
    /// The directory is `0700` and the staged helper carries the source's
    /// executable mode, which is exactly what `verify_trusted_helper` requires:
    /// an owner-only ancestor chain, a regular file, and an executable mode.
    /// Trust is therefore satisfied by construction, which is part of the
    /// contract under test rather than an accident.
    ///
    /// The staged helper is a hard link to `source` rather than a copy of it. A
    /// link shares the source's inode and never opens it for writing, so the
    /// inode's write access is never taken and the kernel will not refuse to
    /// execute it. That is the difference between a refusal that cannot happen
    /// and one that is merely unlikely. It also removes a per-fixture copy of
    /// the whole binary from every test.
    ///
    /// A hard link needs the source and the fixture to share a filesystem. The
    /// system temporary directory is tried first, because its ancestor chain is
    /// the one `verify_trusted_helper` trusts by construction; a directory next
    /// to the source is the fallback, because it is the same filesystem by
    /// definition. When neither can be linked, staging fails loudly rather than
    /// falling back to a copy, because a copy is exactly the construction that
    /// can be refused.
    pub fn stage(source: &Path) -> Self {
        let source = trusted_source(source);
        let executable = fs::metadata(&source)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
        assert!(
            executable,
            "the sandbox helper source {} is not executable, so no trusted helper can be \
             staged from it",
            source.display()
        );
        let mut failure: Option<io::Error> = None;
        for parent in [None, source.parent()] {
            let directory = match parent {
                None => tempfile::tempdir(),
                Some(parent) => tempfile::tempdir_in(parent),
            };
            let Ok(directory) = directory else {
                failure = Some(io::Error::other("no fixture directory could be created"));
                continue;
            };
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("owner-only fixture directory");
            let helper = directory.path().join(HELPER_NAME);
            match fs::hard_link(&source, &helper) {
                Ok(()) => return Self { directory, helper },
                Err(error) => failure = Some(error),
            }
        }
        panic!(
            "cannot stage a trusted helper from {}: {failure:?}. A copy is deliberately not \
             used as a fallback: a file that was written can be refused with ETXTBSY, which is \
             the failure this fixture exists to make impossible.",
            source.display()
        );
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
    /// is a hard link shared with the Cargo-built helper and with other
    /// fixtures, and writing through it would reach all of them.
    ///
    /// The published replacement is a file that was written, so executing it
    /// may be refused with `ETXTBSY` instead of failing as a malformed
    /// executable. Both are loud failures, and the junk fixture is never
    /// expected to run successfully.
    pub fn replace_with_junk(&self, bytes: &[u8]) {
        let staging = self.staging_path();
        fs::write(&staging, bytes).expect("write the replacement helper");
        self.publish(staging, 0o755);
    }

    /// Change only this fixture's helper mode, so a trust rejection can be
    /// proven without touching any other fixture's bytes.
    ///
    /// Like `replace_with_junk`, this publishes a replacement rather than
    /// changing the inode: the staged helper may be shared, and a `chmod` on a
    /// shared inode would be visible to every fixture that links it.
    pub fn set_mode(&self, mode: u32) {
        let staging = self.staging_path();
        fs::copy(&self.helper, &staging).expect("copy the staged helper");
        self.publish(staging, mode);
    }

    /// The staging name a replacement is written under, inside this fixture's
    /// own directory, so that the final `rename` is a same-filesystem atomic
    /// swap — the only replacement form Linux accepts for an executable that
    /// may be running.
    fn staging_path(&self) -> PathBuf {
        self.directory.path().join(format!("{HELPER_NAME}.staging"))
    }

    /// Publish `staging` as this fixture's helper, atomically.
    fn publish(&self, staging: PathBuf, mode: u32) {
        fs::set_permissions(&staging, fs::Permissions::from_mode(mode))
            .expect("stage the replacement mode");
        fs::rename(&staging, &self.helper).expect("publish the replacement helper");
    }
}

/// Clear the group and world write bits of `path`, if it has any.
///
/// Returns whether the mode was changed.
///
/// A hard-linked helper carries the source's mode, and `verify_trusted_helper`
/// rejects a helper that anyone but its owner may write. Cargo derives the
/// executable's mode from the process umask — `0666 & ~umask` plus the execute
/// bits — so a permissive umask (0002 is common in shared development
/// environments) leaves the build output group-writable and no fixture could
/// stage it by linking.
///
/// The change is deliberately one-directional: it removes write permission for
/// everyone else and keeps the owner's permissions, the execute bits, and every
/// other bit exactly as they were. It never makes a file more writable, so a
/// fixture that stages a link can only become *more* trusted because of it, and
/// the build output's bytes are never touched.
pub fn normalize_trusted_mode(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let mode = metadata.permissions().mode();
    if mode & 0o022 == 0 {
        return false;
    }
    let trusted = mode & 0o7777 & !0o022;
    fs::set_permissions(path, fs::Permissions::from_mode(trusted))
        .expect("normalize the build output's mode");
    true
}

/// The source of a staged helper, with its mode normalized once.
///
/// The normalization happens under a lock, before the first link is made, so
/// that no fixture can stage a helper whose mode has not been settled yet: a
/// fixture that linked before the normalization would hold a group-writable
/// helper, and the trust check would reject it.
fn trusted_source(source: &Path) -> PathBuf {
    static TRUSTED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let trusted = TRUSTED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut seen = trusted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = source.to_path_buf();
    if seen.contains(&key) {
        return key;
    }
    normalize_trusted_mode(source);
    seen.insert(key.clone());
    key
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
