use eggwork_runner::{LocalProcessRunner, TrustedLandlockSetup};
use std::{fs, path::PathBuf};

fn helper_in_target_dir() -> Option<PathBuf> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("OUT_DIR").map(PathBuf::from))?;
    let candidate = target.join("debug").join("eggwork-sandbox-helper");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn place_trusted_helper() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let source = helper_in_target_dir()?;
    let helper_dir = tempfile::tempdir().unwrap();
    let helper = helper_dir.path().join("eggwork-sandbox-helper");
    fs::copy(&source, &helper).ok()?;
    fs::set_permissions(helper_dir.path(), fs::Permissions::from_mode(0o700)).ok()?;
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).ok()?;
    // Leak the directory so its helper keeps living until process exit;
    // `discover_sibling()` reads trust only at probe time, so this is safe.
    let leaked = Box::leak(Box::new(helper_dir));
    Some(leaked.path().join("eggwork-sandbox-helper"))
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn trusted_helper_advertises_landlock_workspace_rw_capability() {
    let Some(helper) = place_trusted_helper() else {
        // No sandbox-helper binary located; skip on this host.
        eprintln!(
            "skipping trusted_helper_advertises_landlock_workspace_rw_capability: helper binary not found"
        );
        return;
    };
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(helper));
    let capabilities = runner.execution_capabilities().await;
    assert!(
        capabilities
            .iter()
            .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
        "expected Landlock workspace-rw capability on a kernel with ABI V4; got {capabilities:?}"
    );
}

#[tokio::test]
async fn missing_helper_advertises_no_landlock_capability() {
    let runner = LocalProcessRunner::new(TrustedLandlockSetup::new(PathBuf::from(
        "/nonexistent/eggwork-sandbox-helper",
    )));
    let capabilities = runner.execution_capabilities().await;
    assert!(
        !capabilities
            .iter()
            .any(|feature| feature == "isolation.landlock.workspace-rw.v1"),
        "missing helper must not advertise Landlock capability; got {capabilities:?}"
    );
}
