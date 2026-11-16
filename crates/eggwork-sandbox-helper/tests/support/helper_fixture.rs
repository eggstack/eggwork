//! Shared test fixture for staging a trusted sandbox helper.
//!
//! Hard-link staging avoids Linux `ETXTBSY` by never opening the executable for
//! writing. The fixture normalizes only unsafe mode bits, uses private paths,
//! and publishes test replacements atomically. This module is test-only.

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
    /// Stage `source` in a private directory accepted by `verify_trusted_helper`.
    ///
    /// A hard link avoids `ETXTBSY` by not writing the executable. Try the temp
    /// directory, then the source's parent for a shared filesystem; fail rather
    /// than fall back to a copy.
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

    /// Publish junk by atomic replacement; writing in place would modify every
    /// fixture sharing the staged helper's hard link.
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

/// Remove group/world write bits so the linked helper passes its trust check.
///
/// The change is one-way and preserves owner, execute, and other mode bits; it
/// never changes the build output's bytes.
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
