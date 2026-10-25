//! The Windows service-host contract.
//!
//! This target is Windows-only and is run explicitly on `windows-latest`,
//! not through `--workspace --all-targets`: the server's own unit tests are
//! `/bin/sh`-shaped and are not claimed as Windows coverage, and this
//! contract is about the Windows surface specifically.
//!
//! The contract is a *contract*, not an installation. Nothing here mutates
//! the service control manager: the ownership comparison is exercised
//! through Eggup's own classification, and the manager is only constructed,
//! never run against a real registration. Mutating a real registration is
//! the hosted lifecycle job's job, and it is a different kind of evidence.

#![cfg(windows)]

use eggup_service::{Ownership, RegistrationSnapshot};
use eggwork_server::deployment;
use std::path::Path;

const EXECUTABLE: &str = r"C:\Program Files\Eggwork\bin\eggworkd.exe";
const CONFIG: &str = r"C:\ProgramData\Eggwork\node.json";

fn spec() -> eggup_service::ServiceSpec {
    deployment::service_spec_for_install(Path::new(EXECUTABLE), Some(Path::new(CONFIG)))
        .expect("the service spec is valid")
}

/// A registration that matches the spec exactly.
fn owned_registration(spec: &eggup_service::ServiceSpec) -> RegistrationSnapshot {
    RegistrationSnapshot {
        present: true,
        executable: Some(spec.executable().to_path_buf()),
        args: spec.args().to_vec(),
        config: spec.config().map(Path::to_path_buf),
        malformed: false,
    }
}

#[test]
fn the_windows_service_spec_names_the_service_host() {
    let spec = spec();
    // Windows launches the daemon through the service control dispatcher, so
    // the registered argv names the service host explicitly. `run` would
    // start a foreground node that never connects back to the manager.
    assert_eq!(
        spec.args(),
        &[
            "service-host".to_owned(),
            "--config".to_owned(),
            CONFIG.to_owned(),
        ]
    );
    assert_eq!(spec.id().as_str(), deployment::SERVICE_ID);
    assert_eq!(spec.executable(), Path::new(EXECUTABLE));
    assert_eq!(spec.config(), Some(Path::new(CONFIG)));
}

#[test]
fn the_service_identity_is_exact() {
    let spec = spec();
    assert_eq!(spec.executable(), Path::new(EXECUTABLE));
    assert_eq!(spec.config(), Some(Path::new(CONFIG)));
    // The service identity is the one the manager registers under.
    assert_eq!(spec.id().as_str(), "eggwork-node");
}

#[test]
fn a_matching_registration_is_owned() {
    let spec = spec();
    assert_eq!(
        owned_registration(&spec).ownership(&spec),
        Ownership::Owned,
        "an exact registration must be owned, or the manager refuses to manage it"
    );
}

#[test]
fn a_different_executable_is_not_owned() {
    let spec = spec();
    let mut registration = owned_registration(&spec);
    registration.executable = Some(Path::new(r"C:\elsewhere\eggworkd.exe").to_path_buf());
    assert_eq!(registration.ownership(&spec), Ownership::Foreign);
}

#[test]
fn a_different_config_is_not_owned() {
    let spec = spec();
    let mut registration = owned_registration(&spec);
    registration.config = Some(Path::new(r"C:\ProgramData\Eggwork\other.json").to_path_buf());
    assert_eq!(registration.ownership(&spec), Ownership::Foreign);
}

#[test]
fn a_different_critical_argv_is_not_owned() {
    let spec = spec();
    let mut registration = owned_registration(&spec);
    // The foreground verb under the same executable and config is a
    // different deployment: it would start a node that never answers the
    // manager.
    registration.args = vec!["run".to_owned(), "--config".to_owned(), CONFIG.to_owned()];
    assert_eq!(registration.ownership(&spec), Ownership::Foreign);
}

#[test]
fn an_absent_registration_is_not_owned() {
    let spec = spec();
    assert_eq!(
        RegistrationSnapshot::absent().ownership(&spec),
        Ownership::Absent
    );
}

#[test]
fn the_start_type_policy_is_bounded() {
    use eggup_service::WindowsStartType;
    assert_eq!(
        deployment::parse_windows_start_type(None).ok(),
        Some(WindowsStartType::Manual),
        "a service installed without an explicit start type must not start on boot"
    );
    assert_eq!(
        deployment::parse_windows_start_type(Some("automatic")).ok(),
        Some(WindowsStartType::Automatic)
    );
    assert_eq!(
        deployment::parse_windows_start_type(Some("manual")).ok(),
        Some(WindowsStartType::Manual)
    );
    assert_eq!(
        deployment::parse_windows_start_type(Some("disabled")).ok(),
        Some(WindowsStartType::Disabled)
    );
    for refused in ["", "auto", "AUTOMATIC", "Manual", "manual ", "yes"] {
        let error = deployment::parse_windows_start_type(Some(refused))
            .expect_err("an unknown start type must be refused");
        assert_eq!(error.kind(), "invalid");
    }
}

#[test]
fn the_windows_manager_path_is_reachable() {
    use eggup_service::WindowsStartType;
    // The manager exists only on Windows, and it is constructed only for the
    // three named start types. Constructing it proves the typed adapter is
    // reachable from the product surface; mutating a registration is the
    // hosted lifecycle job's job.
    for start_type in [
        WindowsStartType::Automatic,
        WindowsStartType::Manual,
        WindowsStartType::Disabled,
    ] {
        let manager =
            deployment::windows_scm_manager(start_type, std::time::Duration::from_secs(60))
                .expect("the SCM policy is valid");
        // The manager is Eggup's: it is the only service-manager
        // implementation the workspace depends on.
        let _manager: Box<dyn eggup_service::ServiceManager> = Box::new(manager);
    }
}
