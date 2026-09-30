//! Static producer/consumer parity between Eggpack release facts and Eggwork
//! deployment facts (Operations M003).
//!
//! Eggpack owns the checked-in release contract, manifests, installers, and
//! generated release CI. Eggwork owns which binaries constitute one installed
//! node and how they are moved together. This suite reconciles the two sets of
//! facts using only the checked-in static configuration and Eggwork's own
//! deployment constants, so neither library takes a runtime dependency on the
//! other. Eggpack's own schema validation, expansion, and workflow drift checks
//! run through the pinned `eggpack` tool in CI; this suite proves the declared
//! configuration still says what Eggwork means.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use eggwork_server::deployment::{
    DEST_DAEMON, DEST_HELPER, MEMBER_DAEMON, MEMBER_HELPER, PRODUCT_ID, SERVICE_ID,
    install_unit_matrix,
};
use serde_json::Value as Json;
use toml::Value as Toml;

const LINUX_TARGETS: [&str; 2] = ["aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"];
const DARWIN_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
const WINDOWS_TARGETS: [&str; 1] = ["x86_64-pc-windows-msvc"];
/// The Windows release identity keeps the platform executable suffix.
const WINDOWS_DAEMON: &str = "eggworkd.exe";
const SAMPLE_VERSION: &str = "9.9.9";
/// Generated jobs that verify the exact checked-out release source: five
/// builds, five qualifications, five consumer validations, the required-evidence
/// gate, aggregation, and draft staging.
const SOURCE_VERIFYING_JOBS: usize = 18;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("read {relative}: {error}"));
    assert!(
        bytes.len() <= 1 << 20,
        "{relative} exceeds the static-size bound"
    );
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("decode {relative}: {error}"))
}

fn release_toml(name: &str) -> Toml {
    toml::from_str(&read(&format!("release/eggpack/{name}"))).unwrap_or_else(|error| {
        panic!("release/eggpack/{name} is not valid TOML: {error}");
    })
}

fn release_json(name: &str) -> Json {
    serde_json::from_str(&read(&format!("release/eggpack/{name}")))
        .unwrap_or_else(|error| panic!("release/eggpack/{name} is not valid JSON: {error}"))
}

fn toml_to_json(value: &Toml) -> Json {
    serde_json::to_value(value).expect("TOML value is JSON representable")
}

fn contract() -> Toml {
    release_toml("distribution.toml")
}

fn contract_targets() -> Vec<Toml> {
    contract()["targets"]
        .as_array()
        .expect("contract declares a target array")
        .clone()
}

fn target(triple: &str) -> Toml {
    contract_targets()
        .into_iter()
        .find(|entry| entry["triple"].as_str() == Some(triple))
        .unwrap_or_else(|| panic!("contract publishes {triple}"))
}

fn expand(template: &str, product: &str, version: &str, triple: &str) -> String {
    template
        .replace("{product}", product)
        .replace("{version}", version)
        .replace("{target}", triple)
}

/// Every published file for one target at one version, as `(asset, install)`.
fn published(entry: &Toml, product: &str, version: &str, triple: &str) -> Vec<(String, String)> {
    let asset = &entry["asset"];
    match asset["kind"].as_str() {
        Some("direct") => vec![(
            expand(
                asset["asset"].as_str().expect("direct asset template"),
                product,
                version,
                triple,
            ),
            expand(
                asset["install"].as_str().expect("direct install identity"),
                product,
                version,
                triple,
            ),
        )],
        Some("bundle") => asset["entries"]
            .as_array()
            .expect("bundle declares an entry array")
            .iter()
            .map(|entry| {
                (
                    expand(
                        entry["asset"].as_str().expect("entry asset template"),
                        product,
                        version,
                        triple,
                    ),
                    expand(
                        entry["install"].as_str().expect("entry install identity"),
                        product,
                        version,
                        triple,
                    ),
                )
            })
            .collect(),
        other => panic!("{triple} declares unsupported asset kind {other:?}"),
    }
}

fn install_identities(triple: &str) -> Vec<String> {
    published(&target(triple), PRODUCT_ID, SAMPLE_VERSION, triple)
        .into_iter()
        .map(|(_, install)| install)
        .collect()
}

fn asset_names(triple: &str) -> Vec<String> {
    published(&target(triple), PRODUCT_ID, SAMPLE_VERSION, triple)
        .into_iter()
        .map(|(asset, _)| asset)
        .collect()
}

fn required_triples() -> Vec<String> {
    let mut triples: Vec<String> = LINUX_TARGETS
        .iter()
        .chain(DARWIN_TARGETS.iter())
        .chain(WINDOWS_TARGETS.iter())
        .map(|triple| (*triple).to_owned())
        .collect();
    triples.sort();
    triples
}

fn build_bindings() -> BTreeMap<String, Json> {
    let bindings = release_toml("build-bindings.toml");
    bindings["targets"]
        .as_table()
        .expect("build bindings are keyed by canonical triple")
        .iter()
        .map(|(triple, entries)| {
            (
                triple.clone(),
                toml_to_json(&Toml::Array(
                    entries
                        .as_array()
                        .expect("each target binds a list")
                        .clone(),
                )),
            )
        })
        .collect()
}

#[test]
fn contract_publishes_exactly_the_five_canonical_targets() {
    let contract = contract();
    assert_eq!(contract["schema_version"].as_integer(), Some(1));
    assert_eq!(contract["product"]["id"].as_str(), Some(PRODUCT_ID));

    let mut published_triples: Vec<String> = contract_targets()
        .iter()
        .map(|entry| entry["triple"].as_str().expect("triple").to_owned())
        .collect();
    published_triples.sort();
    assert_eq!(published_triples, required_triples());

    let mut seen_aliases = BTreeSet::new();
    for entry in contract_targets() {
        let aliases = entry["aliases"]
            .as_array()
            .expect("every target declares a finite alias list")
            .iter()
            .map(|alias| alias.as_str().expect("alias").to_owned())
            .collect::<Vec<_>>();
        assert!(
            !aliases.is_empty() && aliases.len() <= 4,
            "alias list is finite"
        );
        for alias in aliases {
            assert!(
                seen_aliases.insert(alias.clone()),
                "alias {alias} is declared more than once"
            );
        }
    }
    assert_eq!(
        seen_aliases,
        BTreeSet::from([
            "linux-x64".to_owned(),
            "linux-arm64".to_owned(),
            "macos-x64".to_owned(),
            "macos-arm64".to_owned(),
            "windows-x64".to_owned(),
        ])
    );
}

#[test]
fn every_contract_alias_resolves_to_its_own_triple() {
    for entry in contract_targets() {
        let triple = entry["triple"].as_str().expect("triple").to_owned();
        for alias in entry["aliases"].as_array().expect("aliases") {
            let alias = alias.as_str().expect("alias").to_owned();
            let entries = contract_targets();
            let owners: Vec<String> = entries
                .iter()
                .filter(|candidate| {
                    candidate["aliases"]
                        .as_array()
                        .is_some_and(|list| list.iter().any(|a| a.as_str() == Some(&alias)))
                })
                .map(|candidate| candidate["triple"].as_str().expect("triple").to_owned())
                .collect();
            assert_eq!(owners, vec![triple.clone()], "alias {alias} is unambiguous");
        }
    }
}

#[test]
fn linux_targets_publish_one_two_member_bundle() {
    for triple in LINUX_TARGETS {
        let entry = target(triple);
        assert_eq!(entry["asset"]["kind"].as_str(), Some("bundle"), "{triple}");
        assert_eq!(
            entry["asset"]["entries"].as_array().map(Vec::len),
            Some(2),
            "{triple} ships exactly the daemon and the helper"
        );
        assert_eq!(
            install_identities(triple),
            vec![MEMBER_DAEMON.to_owned(), MEMBER_HELPER.to_owned()],
            "{triple} install identities are the deployment unit members"
        );
    }
}

#[test]
fn non_linux_targets_publish_the_daemon_directly() {
    for triple in DARWIN_TARGETS {
        let entry = target(triple);
        assert_eq!(entry["asset"]["kind"].as_str(), Some("direct"), "{triple}");
        assert_eq!(install_identities(triple), vec![MEMBER_DAEMON.to_owned()]);
    }
    let windows = WINDOWS_TARGETS[0];
    assert_eq!(target(windows)["asset"]["kind"].as_str(), Some("direct"));
    assert_eq!(install_identities(windows), vec![WINDOWS_DAEMON.to_owned()]);
}

#[test]
fn windows_publishes_an_exe_suffixed_executable() {
    let assets = asset_names(WINDOWS_TARGETS[0]);
    assert_eq!(assets.len(), 1);
    assert!(
        assets[0].ends_with(".exe"),
        "{assets:?} must be a Windows image"
    );
    assert!(
        !MEMBER_DAEMON.ends_with(".exe"),
        "the member identity is stable"
    );
    assert_eq!(WINDOWS_DAEMON, format!("{MEMBER_DAEMON}.exe"));
    for triple in LINUX_TARGETS.iter().chain(DARWIN_TARGETS.iter()) {
        for asset in asset_names(triple) {
            assert!(
                !asset.ends_with(".exe"),
                "{triple} must not publish a Windows image"
            );
        }
    }
}

#[test]
fn every_release_file_has_an_exact_sha256_sidecar() {
    let mut sidecars = BTreeSet::new();
    let mut assets = BTreeSet::new();
    for triple in required_triples() {
        let template = target(&triple)["checksum"]["sidecar"]
            .as_str()
            .expect("every target declares a checksum template")
            .to_owned();
        assert_eq!(template, "{asset}.sha256", "{triple} sidecar template");
        let published_assets = asset_names(&triple);
        assert!(!published_assets.is_empty(), "{triple} publishes nothing");
        for asset in published_assets {
            assert!(
                asset.ends_with(&format!("-{triple}"))
                    || asset.ends_with(&format!("-{triple}.exe")),
                "{asset} does not carry its canonical target {triple}"
            );
            assert!(
                sidecars.insert(template.replace("{asset}", &asset)),
                "the sidecar of {asset} is not unique"
            );
            assert!(assets.insert(asset), "release asset names must be unique");
        }
    }
    // Two two-member Linux bundles plus three single-artifact targets.
    assert_eq!(assets.len(), 7);
    assert_eq!(sidecars.len(), 7);
}

#[test]
fn expanded_release_names_are_unique_under_case_insensitive_comparison() {
    let mut files: BTreeSet<String> = BTreeSet::new();
    for triple in required_triples() {
        for (asset, install) in published(&target(&triple), PRODUCT_ID, SAMPLE_VERSION, &triple) {
            assert!(
                files.insert(asset.to_ascii_lowercase()),
                "release file {asset} collides under ASCII case folding"
            );
            let mut within_target: BTreeSet<String> = BTreeSet::new();
            assert!(
                within_target.insert(install.to_ascii_lowercase()),
                "install identity {install} repeats within {triple}"
            );
        }
    }
    // Two two-member Linux bundles plus three single-artifact targets. The
    // daemon install identity is deliberately reused across targets; only
    // release file names must be globally unique.
    assert_eq!(files.len(), 7);
}

#[test]
fn install_identities_match_the_deployment_install_unit() {
    let configured = install_unit_matrix(true);
    assert_eq!(configured.len(), 2);
    let expected = vec![
        (MEMBER_DAEMON.to_owned(), DEST_DAEMON.to_owned(), true),
        (MEMBER_HELPER.to_owned(), DEST_HELPER.to_owned(), true),
    ];
    for (member, expected) in configured.iter().zip(expected) {
        assert_eq!(member.member, expected.0);
        assert_eq!(member.destination, expected.1);
        assert_eq!(member.required, expected.2);
    }
    assert_eq!(SERVICE_ID, "eggwork-node");
    assert!(
        DEST_DAEMON.ends_with(MEMBER_DAEMON) && DEST_HELPER.ends_with(MEMBER_HELPER),
        "Eggup destinations keep the member identity as their file name"
    );
    assert_eq!(MEMBER_DAEMON, "eggworkd");
    assert_eq!(MEMBER_HELPER, "eggwork-sandbox-helper");
}

#[test]
fn the_helper_ships_exactly_on_linux_release_targets() {
    let shipping: Vec<String> = required_triples()
        .into_iter()
        .filter(|triple| install_identities(triple).contains(&MEMBER_HELPER.to_owned()))
        .collect();
    assert_eq!(shipping, LINUX_TARGETS.map(str::to_owned).to_vec());
    assert_eq!(
        install_unit_matrix(false)
            .into_iter()
            .find(|member| member.member == MEMBER_HELPER)
            .map(|member| member.required),
        Some(false),
        "a non-Linux install unit must not require the Landlock helper"
    );
}

#[test]
fn the_daemon_ships_on_every_release_target() {
    for triple in required_triples() {
        let identities = install_identities(&triple);
        let expected = if WINDOWS_TARGETS.contains(&triple.as_str()) {
            WINDOWS_DAEMON
        } else {
            MEMBER_DAEMON
        };
        assert_eq!(
            identities,
            if LINUX_TARGETS.contains(&triple.as_str()) {
                vec![MEMBER_DAEMON.to_owned(), MEMBER_HELPER.to_owned()]
            } else {
                vec![expected.to_owned()]
            },
            "{triple} must ship the daemon"
        );
    }
}

#[test]
fn no_release_asset_carries_node_state() {
    let forbidden = [
        "node.json",
        "config",
        "database",
        "sqlite",
        "workspace",
        "blob",
        "artifact",
        "drain",
        "journal",
    ];
    for triple in required_triples() {
        for (asset, install) in published(&target(&triple), PRODUCT_ID, SAMPLE_VERSION, &triple) {
            let folded = format!("{} {install}", asset.to_ascii_lowercase());
            for token in forbidden {
                assert!(
                    !folded.contains(token),
                    "{triple} publishes node state in {asset}/{install}"
                );
            }
        }
    }
}

#[test]
fn the_linux_helper_and_daemon_share_one_release_identity() {
    for triple in LINUX_TARGETS {
        let entry = target(triple);
        // One contract target, one asset form, one set of install identities:
        // the pair cannot be selected from two different releases because the
        // contract expresses them as members of a single target entry.
        let assets = entry["asset"]["entries"].as_array().map(Vec::len);
        assert_eq!(assets, Some(2));
        assert!(entry["asset"].get("members").is_none());
        assert!(entry["asset"].get("archive").is_none());
        let versions: Vec<String> = published(&entry, PRODUCT_ID, SAMPLE_VERSION, triple)
            .into_iter()
            .map(|(asset, _)| asset)
            .map(|asset| {
                let suffix = asset.trim_start_matches("eggwork");
                suffix.to_owned()
            })
            .collect();
        for asset in &versions {
            assert!(asset.contains(&format!("-{SAMPLE_VERSION}-")), "{asset}");
        }
    }
}

#[test]
fn pack_policy_builds_and_qualifies_every_target_natively() {
    let pack = release_toml("pack.toml");
    assert_eq!(pack["schema_version"].as_integer(), Some(1));
    let targets = pack["targets"].as_array().expect("pack targets");
    let expected = required_triples();
    assert_eq!(targets.len(), expected.len());
    let mut triples: Vec<String> = Vec::new();
    for policy in targets {
        let triple = policy["target"]
            .as_str()
            .expect("pack target triple")
            .to_owned();
        triples.push(triple.clone());
        assert_eq!(
            policy["strategy"].as_str(),
            Some("native_cargo"),
            "{triple}"
        );
        assert_eq!(
            policy["qualification"].as_str(),
            Some("native"),
            "{triple} is natively qualified"
        );
        assert_eq!(policy["support"].as_str(), Some("required"), "{triple}");
        assert!(
            policy.get("qualification_host").is_none(),
            "{triple} qualifies on its build host"
        );
        assert_eq!(
            policy["floor"]["kind"].as_str(),
            Some("none"),
            "{triple} declares no compatibility floor"
        );
        assert_eq!(policy["toolchain"]["rust"].as_str(), Some("1.89.0"));
        assert!(
            policy["toolchain"].get("cargo_zigbuild").is_none()
                && policy["toolchain"].get("zig").is_none(),
            "{triple} uses native cargo, so no cross tool is configured"
        );
    }
    assert_eq!(triples, expected, "pack targets are canonically ordered");
}

#[test]
fn toolchain_policy_matches_the_checked_in_rust_toolchain() {
    let manifest = read("rust-toolchain.toml");
    assert!(
        manifest.contains(r#"channel = "1.89.0""#),
        "the exact toolchain patch is the packaging policy: {manifest}"
    );
}

#[test]
fn build_bindings_name_exact_packages_and_binaries() {
    let bindings = build_bindings();
    assert_eq!(bindings.len(), required_triples().len());
    for triple in required_triples() {
        let outputs = bindings
            .get(&triple)
            .unwrap_or_else(|| panic!("{triple} has explicit build bindings"))
            .as_array()
            .expect("binding list");
        let pairs: Vec<(String, String)> = outputs
            .iter()
            .map(|output| {
                (
                    output["package"].as_str().expect("package").to_owned(),
                    output["binary"].as_str().expect("binary").to_owned(),
                )
            })
            .collect();
        if LINUX_TARGETS.contains(&triple.as_str()) {
            assert_eq!(
                pairs,
                vec![
                    ("eggwork-server".to_owned(), MEMBER_DAEMON.to_owned()),
                    (
                        "eggwork-sandbox-helper".to_owned(),
                        MEMBER_HELPER.to_owned()
                    ),
                ],
                "{triple} binds the daemon then the helper"
            );
            for (index, output) in outputs.iter().enumerate() {
                assert_eq!(output["selector"]["kind"].as_str(), Some("bundle_entry"));
                assert_eq!(output["selector"]["index"].as_i64(), Some(index as i64));
            }
        } else {
            assert_eq!(
                pairs,
                vec![("eggwork-server".to_owned(), MEMBER_DAEMON.to_owned())],
                "{triple} binds only the daemon"
            );
            assert_eq!(outputs[0]["selector"]["kind"].as_str(), Some("direct"));
        }
    }
}

#[test]
fn build_binding_packages_and_binaries_exist_in_this_workspace() {
    let server = read("crates/eggwork-server/Cargo.toml");
    assert!(server.contains(r#"name = "eggwork-server""#));
    assert!(server.contains("[[bin]]"));
    assert!(server.contains(r#"name = "eggworkd""#));
    let helper = read("crates/eggwork-sandbox-helper/Cargo.toml");
    assert!(helper.contains(r#"name = "eggwork-sandbox-helper""#));
    assert!(helper.contains("[[bin]]"));
    for triple in required_triples() {
        for output in build_bindings()[&triple].as_array().expect("bindings") {
            let (package, binary) = (
                output["package"].as_str().expect("package"),
                output["binary"].as_str().expect("binary"),
            );
            let manifest = read(&format!("crates/{package}/Cargo.toml"));
            assert!(
                manifest.contains(&format!("name = \"{binary}\"")),
                "{package} declares a {binary} binary target for {triple}"
            );
        }
    }
}

#[test]
fn core_qualification_smoke_selects_the_daemon_with_a_fixed_argv() {
    let bindings = release_toml("qualification-bindings.toml");
    assert_eq!(bindings["schema_version"].as_integer(), Some(1));
    assert_eq!(
        bindings["targets"].as_table().map(|targets| targets.len()),
        Some(required_triples().len())
    );
    for triple in required_triples() {
        let smoke = &bindings["targets"][&triple]["smoke"];
        let expected = if LINUX_TARGETS.contains(&triple.as_str()) {
            ("bundle_entry", Some(0))
        } else {
            ("direct", None)
        };
        assert_eq!(
            smoke["selector"]["kind"].as_str(),
            Some(expected.0),
            "{triple}"
        );
        assert_eq!(
            smoke["selector"].get("index").and_then(Toml::as_integer),
            expected.1,
            "{triple}"
        );
        assert_eq!(
            smoke["argv"].as_array().map(|argv| {
                argv.iter()
                    .map(|value| value.as_str().unwrap_or_default().to_owned())
                    .collect::<Vec<_>>()
            }),
            Some(vec!["version".to_owned()]),
            "{triple} smoke is the fixed configuration-free version command"
        );
        assert_eq!(smoke["timeout_ms"].as_integer(), Some(10_000));
        assert_eq!(smoke["stdout_limit"].as_integer(), Some(8192));
        assert_eq!(smoke["stderr_limit"].as_integer(), Some(8192));
    }
}

#[test]
fn the_qualification_selector_always_names_a_bound_build_output() {
    let bindings = release_toml("qualification-bindings.toml");
    let built = build_bindings();
    for triple in required_triples() {
        let smoke = toml_to_json(&bindings["targets"][&triple]["smoke"]["selector"]);
        let matched = built[&triple]
            .as_array()
            .expect("binding list")
            .iter()
            .any(|output| output["selector"] == smoke);
        assert!(matched, "{triple} smoke must select a bound build output");
    }
}

#[test]
fn consumer_validation_is_gating_and_selects_the_linux_helper() {
    let validators = release_json("consumer-validators.json");
    assert_eq!(
        validators.as_object().map(|map| map.len()),
        Some(required_triples().len())
    );
    for triple in required_triples() {
        let validator = &validators[&triple];
        assert_eq!(validator["schema_version"].as_i64(), Some(1));
        assert_eq!(
            validator["interpreter"].as_str(),
            Some("python3"),
            "{triple} has no arbitrary interpreter"
        );
        let (expected_kind, expected_index, expected_script) =
            if LINUX_TARGETS.contains(&triple.as_str()) {
                (
                    "bundle_entry",
                    Some(1),
                    "scripts/validate-sandbox-helper.py",
                )
            } else {
                ("direct", None, "scripts/validate-daemon-version.py")
            };
        assert_eq!(
            validator["selector"]["kind"].as_str(),
            Some(expected_kind),
            "{triple} validator selector"
        );
        assert_eq!(validator["selector"]["index"].as_i64(), expected_index);
        assert_eq!(validator["script"].as_str(), Some(expected_script));
        let script = validator["script"].as_str().expect("script");
        assert!(repo_root().join(script).is_file(), "{script} is checked in");
        assert!(validator.get("env").is_none() && validator.get("shell").is_none());
        assert!(validator.get("args").is_none() && validator.get("command").is_none());
        assert!(
            (1000..=600_000).contains(&validator["timeout_ms"].as_i64().unwrap_or(0)),
            "{triple} validator timeout is bounded"
        );
    }
}

#[test]
fn the_linux_helper_validator_agrees_with_deployment_version_policy() {
    let helper_source = read("crates/eggwork-sandbox-helper/src/main.rs");
    let deployment = read("crates/eggwork-server/src/deployment.rs");
    assert!(
        helper_source.contains(r#"println!("{}", env!("CARGO_PKG_VERSION"));"#),
        "the helper reports exactly one bare version line"
    );
    assert!(
        deployment.contains(r#"if found != expected_version {"#),
        "deployment compares the trimmed helper version for exact equality"
    );
    let probe = read("scripts/release_candidate_probe.py");
    assert!(probe.contains("shell=False"));
    for forbidden in ["socket", "urllib", "http.client", "requests"] {
        assert!(!probe.contains(forbidden), "the probe must stay offline");
    }
}

#[test]
fn install_policy_marks_every_bundle_member_executable_and_nothing_else() {
    let policy = release_toml("install-policy.toml");
    assert_eq!(policy["schema_version"].as_integer(), Some(1));
    let targets = policy["targets"]
        .as_table()
        .expect("install policy targets");
    assert_eq!(targets.len(), LINUX_TARGETS.len());
    for triple in LINUX_TARGETS {
        let modes = &targets[triple]["modes"];
        assert!(
            modes.get("archive_encoding").is_none(),
            "{triple} is a bundle, not an archive"
        );
        let mut names: Vec<&str> = modes
            .as_table()
            .expect("modes")
            .keys()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        assert_eq!(names, vec![MEMBER_HELPER, MEMBER_DAEMON]);
        for mode in modes.as_table().expect("modes").values() {
            assert_eq!(mode.as_str(), Some("executable"), "{triple} members run");
        }
    }
    for triple in DARWIN_TARGETS.iter().chain(WINDOWS_TARGETS.iter()) {
        assert!(
            !targets.contains_key(*triple),
            "{triple} is a direct release and needs no bundle policy"
        );
    }
}

#[test]
fn static_release_configuration_embeds_no_future_release_identity() {
    let version = env!("CARGO_PKG_VERSION");
    for entry in contract_targets() {
        let triple = entry["triple"].as_str().expect("triple");
        let asset = &entry["asset"];
        let mut templates: Vec<String> = Vec::new();
        match asset["kind"].as_str() {
            Some("direct") => {
                templates.push(asset["asset"].as_str().expect("asset").to_owned());
                templates.push(asset["install"].as_str().expect("install").to_owned());
            }
            Some("bundle") => {
                for member in asset["entries"].as_array().expect("entries") {
                    templates.push(member["asset"].as_str().expect("asset").to_owned());
                    templates.push(member["install"].as_str().expect("install").to_owned());
                }
            }
            other => panic!("{triple} declares unsupported asset kind {other:?}"),
        }
        for template in templates {
            assert!(
                !template.contains(version),
                "{triple} hardcodes the release version in {template}"
            );
            if template.contains('{') {
                assert!(
                    template.contains("{version}") && template.contains("{target}"),
                    "{triple} expresses release identity through templates only: {template}"
                );
            }
        }
    }
    let shape = read("release/eggpack/workflow-shape.json");
    for forbidden in [
        "release_id",
        "source_revision",
        "release_tag",
        "sha256\":",
        "digest",
    ] {
        assert!(
            !shape.contains(forbidden),
            "the static workflow shape must not carry {forbidden}"
        );
    }
    let policy = release_json("github-policy.json");
    assert!(
        policy["release_inputs"]["release_plan"]
            .as_str()
            .expect("release plan path")
            .contains("runtime/")
    );
    assert!(
        policy["release_inputs"]["ci_plan"]
            .as_str()
            .expect("ci plan path")
            .contains("runtime/")
    );
    let template = release_json("github-template.json");
    assert!(
        template.get("tag").is_none() && template.get("release_id").is_none(),
        "the draft template is a static template, not a release"
    );
}

#[test]
fn the_embedded_workflow_shape_matches_the_checked_in_configuration() {
    let shape = release_json("workflow-shape.json");
    assert_eq!(shape["schema_version"].as_i64(), Some(1));

    let pack = release_toml("pack.toml");
    let pack_targets: Vec<Json> = pack["targets"]
        .as_array()
        .expect("pack targets")
        .iter()
        .map(toml_to_json)
        .collect();
    assert_eq!(shape["targets"], Json::Array(pack_targets));
    assert_eq!(
        shape["build_bindings"],
        toml_to_json(&release_toml("build-bindings.toml"))
    );
    assert_eq!(
        shape["qualification_bindings"],
        toml_to_json(&release_toml("qualification-bindings.toml"))
    );
    assert_eq!(
        shape["consumer_validators"],
        release_json("consumer-validators.json")
    );

    let policy = release_json("github-policy.json");
    let inputs = &policy["release_inputs"];
    for (key, path) in [
        ("contract", "release/eggpack/distribution.toml"),
        ("build_bindings", "release/eggpack/build-bindings.toml"),
        (
            "qualification_bindings",
            "release/eggpack/qualification-bindings.toml",
        ),
        ("pack_config", "release/eggpack/pack.toml"),
        ("draft_template", "release/eggpack/github-template.json"),
        (
            "consumer_validators",
            "release/eggpack/consumer-validators.json",
        ),
    ] {
        assert_eq!(inputs[key].as_str(), Some(path), "stale {key} path");
    }

    let aliases: Vec<String> = shape["selected_aliases"]
        .as_array()
        .expect("selected aliases")
        .iter()
        .map(|alias| alias.as_str().expect("alias").to_owned())
        .collect();
    let mut contracted: Vec<String> = contract_targets()
        .iter()
        .flat_map(|entry| {
            entry["aliases"]
                .as_array()
                .expect("aliases")
                .iter()
                .map(|alias| alias.as_str().expect("alias").to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    contracted.sort();
    let mut selected = aliases.clone();
    selected.sort();
    assert_eq!(
        selected, contracted,
        "every contracted alias is selected exactly once for resolution"
    );
    assert_eq!(
        shape["staging"]["tag_source"].as_str(),
        Some("dispatch_input")
    );
}

#[test]
fn github_policy_pins_every_runner_action_and_the_tool_immutably() {
    let policy = release_json("github-policy.json");
    for key in [
        "checkout",
        "rust_toolchain",
        "upload_artifact",
        "download_artifact",
    ] {
        let reference = policy[key]["reference"].as_str().expect("action pin");
        let revision = reference.rsplit_once('@').expect("owner/repo@sha").1;
        assert!(
            revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()),
            "{key} must be an immutable 40-hex action pin, got {reference}"
        );
    }
    let tool = &policy["eggpack_tool"];
    assert_eq!(
        tool["repo"].as_str(),
        Some("https://github.com/eggstack/eggpack")
    );
    let revision = tool["revision"].as_str().expect("tool revision");
    assert_eq!(revision.len(), 40);
    assert!(revision.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(tool["package"].as_str(), Some("eggpack-cli"));
    assert!(
        policy.get("cross_tools").is_none(),
        "native cargo needs no cross tools"
    );

    let runners = policy["runners"].as_array().expect("runner mappings");
    assert_eq!(runners.len(), required_triples().len());
    for runner in runners {
        assert!(!runner["cargo_zigbuild"].as_bool().unwrap_or(true));
        assert!(!runner["zig"].as_bool().unwrap_or(true));
        assert!(
            runner["label"]
                .as_str()
                .is_some_and(|label| !label.is_empty())
        );
    }
    let expected_labels: BTreeSet<&str> = BTreeSet::from([
        "ubuntu-latest",
        "ubuntu-24.04-arm",
        "macos-15-intel",
        "macos-14",
        "windows-latest",
    ]);
    let labels: BTreeSet<&str> = runners
        .iter()
        .map(|runner| runner["label"].as_str().expect("label"))
        .collect();
    assert_eq!(labels, expected_labels);
}

#[test]
fn the_github_draft_policy_is_draft_only_and_needs_no_wrapper() {
    let staging = &release_json("github-policy.json")["staging"];
    assert_eq!(staging["owner"].as_str(), Some("eggstack"));
    assert_eq!(staging["repository"].as_str(), Some("eggwork"));
    assert_eq!(staging["tag_source"].as_str(), Some("dispatch_input"));
    assert_eq!(
        staging["inputs"]["install_policy"].as_str(),
        Some("release/eggpack/install-policy.toml")
    );
    assert_eq!(
        staging["inputs"]["contract"].as_str(),
        Some("release/eggpack/distribution.toml")
    );
    assert!(
        staging["inputs"].get("installer_presentation").is_none(),
        "Eggwork has no product installer wrappers; generated installers are the only presentation"
    );
    let template = release_json("github-template.json");
    assert_eq!(template["token_env"].as_str(), Some("GITHUB_TOKEN"));
    assert!(template.get("make_latest").is_none());
    assert!(
        template["body"]
            .as_str()
            .is_some_and(|body| !body.is_empty()),
        "the draft carries fixed bounded release notes"
    );
}

#[test]
fn the_generated_workflow_is_derived_only_from_static_configuration() {
    let workflow = read(".github/workflows/release.yml");
    assert!(!workflow.contains(&format!("-{SAMPLE_VERSION}-")));
    assert!(!workflow.contains(env!("CARGO_PKG_VERSION")));
    assert!(
        !workflow.contains("&\"0000000000000000000000000000000000000000\""),
        "no unresolved source revision may be rendered"
    );
    assert!(
        !workflow.contains("release/eggpack/runtime/release-plan.json\""),
        "the static checked-in plan path is replaced by workflow-private storage"
    );
    for target in required_triples() {
        assert!(
            workflow.contains(&format!("'--target' '{target}'")),
            "{target} has a build, qualification, and validation path"
        );
        assert!(
            workflow.contains("'--contract' 'release/eggpack/distribution.toml'"),
            "every job names its contract input explicitly"
        );
    }
}

#[test]
fn the_generated_workflow_grants_contents_write_to_only_the_staging_job() {
    let workflow = read(".github/workflows/release.yml");
    assert_eq!(
        workflow.matches("contents: write").count(),
        1,
        "exactly one job may write repository contents"
    );
    let stage = workflow
        .find("\n  stage:\n")
        .expect("a staging job is generated");
    let write = workflow
        .find("contents: write")
        .expect("the staging job writes");
    assert!(write > stage, "the write grant belongs to the staging job");
    assert!(!workflow.contains("id-token: write"));
    for forbidden in [
        "contents: write-all",
        "packages: write",
        "deployments: write",
        "pull-requests: write",
    ] {
        assert!(!workflow.contains(forbidden));
    }
}

#[test]
fn the_generated_workflow_never_publishes_or_mutates_tags() {
    let workflow = read(".github/workflows/release.yml");
    for forbidden in [
        "--clobber",
        "gh release",
        "make_latest",
        "git tag",
        "git push origin",
        "actions/github-script",
        "curl",
    ] {
        assert!(
            !workflow.contains(forbidden),
            "the release workflow must not contain {forbidden}"
        );
    }
    assert!(workflow.contains("workflow_dispatch:"));
    assert!(workflow.contains("release_tag:"));
    assert!(
        !workflow.contains("\n  push:\n"),
        "staging is manual-dispatch only against an exact existing tag"
    );
    assert_eq!(
        workflow.matches("GITHUB_TOKEN").count(),
        2,
        "the staging token appears only as the staging job environment binding"
    );
    let stage = workflow
        .find("\n  stage:\n")
        .expect("a staging job is generated");
    let token = workflow
        .find("GITHUB_TOKEN")
        .expect("staging token binding");
    assert!(
        token > stage,
        "the token is bound only inside the staging job"
    );
    assert!(
        workflow.contains("GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}"),
        "the token reaches the tool through the environment only"
    );
}

#[test]
fn the_generated_workflow_pins_actions_and_installs_the_pinned_tool() {
    let workflow = read(".github/workflows/release.yml");
    let revision = release_json("github-policy.json")["eggpack_tool"]["revision"]
        .as_str()
        .expect("tool revision")
        .to_owned();
    let install = format!(
        "cargo install --git https://github.com/eggstack/eggpack --rev {revision} --locked eggpack-cli"
    );
    assert!(
        workflow.contains(&install),
        "every job installs the pinned tool"
    );
    assert!(!workflow.contains("--rev main"), "no floating branch pin");
    for line in workflow.lines().filter(|line| line.contains("uses:")) {
        let reference = line
            .trim()
            .trim_start_matches("uses:")
            .trim()
            .trim_matches('"');
        let pinned = reference
            .rsplit_once('@')
            .map(|(_, revision)| revision)
            .unwrap_or_default();
        assert_eq!(
            pinned.len(),
            40,
            "action {reference} must be an immutable revision pin"
        );
    }
}

#[test]
fn the_generated_workflow_propagates_one_runtime_identity_to_every_job() {
    let workflow = read(".github/workflows/release.yml");
    let verifications: Vec<&str> = workflow
        .lines()
        .filter(|line| line.contains("'_verify-source'"))
        .collect();
    assert_eq!(
        verifications.len(),
        SOURCE_VERIFYING_JOBS,
        "every build, qualification, validation, gate, aggregate, and stage job \
         verifies the exact checked-out source"
    );
    for call in &verifications {
        assert!(
            call.contains("'--release-plan' 'eggpack-runtime/release-plan.json'"),
            "source verification reads the invocation-local release plan: {call}"
        );
    }
    assert_eq!(
        workflow.matches("_resolve-release").count(),
        1,
        "the release identity is resolved exactly once per run"
    );
    assert!(workflow.contains("git rev-parse --verify HEAD^{commit}"));
    assert!(workflow.contains("'_validate-consumer'"));
    assert!(workflow.contains("'_evaluate-gate'"));
    assert!(workflow.contains("'_aggregate'"));
    assert!(workflow.contains("'_prepare-stage'"));
    assert!(workflow.contains("'_stage-github-draft'"));
    assert_eq!(
        workflow
            .matches("name: \"eggpack-runtime-identity\"")
            .count(),
        SOURCE_VERIFYING_JOBS + 1,
        "one preflight upload is downloaded by every source-verifying job"
    );
}

#[test]
fn no_second_hand_maintained_release_matrix_exists() {
    let workflows = repo_root().join(".github/workflows");
    let mut release = Vec::new();
    let mut other = Vec::new();
    for entry in std::fs::read_dir(&workflows).expect("workflow directory") {
        let name = entry
            .expect("directory entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name == "release.yml" {
            release.push(name);
        } else {
            other.push(name);
        }
    }
    assert_eq!(release, vec!["release.yml".to_owned()]);
    for name in other {
        let text = std::fs::read_to_string(workflows.join(&name)).expect("workflow text");
        for target in required_triples() {
            assert!(
                !text.contains(&format!("--target '{target}'")),
                "{name} must not carry a second hand-maintained release matrix"
            );
        }
    }
}

#[test]
fn the_deployment_module_never_parses_eggpack_configuration() {
    for file in [
        "src/deployment.rs",
        "src/operations.rs",
        "src/bin/eggworkd.rs",
    ] {
        let text = read(&format!("crates/eggwork-server/{file}"));
        for forbidden in [
            "release/eggpack",
            "ReleaseManifest",
            "release-manifest",
            "DistributionContract",
            "eggpack::",
        ] {
            assert!(
                !text.contains(forbidden),
                "{file} must not depend on Eggpack producer configuration at runtime; \
                 release selection and acquisition stay outside this milestone"
            );
        }
    }
}

#[test]
fn linux_only_dependencies_never_enter_the_non_linux_build_graph() {
    // Run 36784830178 proved this the hard way: `landlock` is Linux-only and
    // its unconditional declaration broke the macOS and Windows daemon builds.
    // Every required non-Linux release target builds `eggwork-server`, so a
    // Linux-only crate in the daemon's unconditional dependency closure is a
    // release-blocking defect on Linux CI. This test keeps the gates explicit.
    for (manifest, gated) in [
        (
            "crates/eggwork-runner/Cargo.toml",
            vec![("landlock", "linux")],
        ),
        (
            "crates/eggwork-sandbox-helper/Cargo.toml",
            vec![("landlock", "linux"), ("nix", "unix")],
        ),
    ] {
        let parsed: Toml =
            toml::from_str(&read(manifest)).unwrap_or_else(|error| panic!("{manifest}: {error}"));
        let unconditional = parsed
            .get("dependencies")
            .and_then(Toml::as_table)
            .cloned()
            .unwrap_or_default();
        let targets = parsed
            .get("target")
            .and_then(Toml::as_table)
            .cloned()
            .unwrap_or_default();
        for (dependency, platform) in gated {
            assert!(
                !unconditional.contains_key(dependency),
                "{manifest} must not depend on {dependency} unconditionally; \
                 it does not compile on non-{platform} release hosts"
            );
            let gated_tables: Vec<&String> = targets
                .iter()
                .filter(|(_, table)| {
                    table
                        .get("dependencies")
                        .and_then(Toml::as_table)
                        .is_some_and(|dependencies| dependencies.contains_key(dependency))
                })
                .map(|(key, _)| key)
                .collect();
            assert_eq!(
                gated_tables.len(),
                1,
                "{manifest} must gate {dependency} behind exactly one platform table"
            );
            assert!(
                gated_tables[0].contains(platform),
                "{manifest} gates {dependency} behind {}, not {platform}",
                gated_tables[0],
            );
        }
    }
}
