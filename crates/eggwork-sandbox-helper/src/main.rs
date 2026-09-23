#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod linux {

    use landlock::{
        ABI, Access, AccessFs, PathBeneath, PathFd, RestrictionStatus, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus,
    };
    use serde::Deserialize;
    use std::{
        collections::HashSet,
        fs,
        io::{Read, Write},
        os::unix::{fs::MetadataExt, net::UnixStream, process::ExitStatusExt},
        path::{Path, PathBuf},
        process::{Command, ExitStatus},
    };

    const MAX_SPEC_BYTES: u64 = 1024 * 1024;
    const MAX_ARG_COUNT: usize = 256;
    const MAX_ARG_BYTES: usize = 32 * 1024;
    const MAX_ENV_COUNT: usize = 256;
    const MAX_ENV_NAME_BYTES: usize = 256;
    const MAX_ENV_VALUE_BYTES: usize = 16 * 1024;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LaunchSpec {
        schema_version: u16,
        profile: String,
        root: PathBuf,
        cwd: PathBuf,
        argv: Vec<String>,
        environment: Vec<(String, String)>,
    }

    pub fn entry() {
        std::process::exit(run().unwrap_or(125));
    }

    fn run() -> Result<i32, ()> {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--spec")) {
            return Err(());
        }
        let spec_path = PathBuf::from(args.next().ok_or(())?);
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--status")) {
            return Err(());
        }
        let status_path = PathBuf::from(args.next().ok_or(())?);
        if args.next().is_some() {
            return Err(());
        }
        let mut status = UnixStream::connect(&status_path).map_err(|_| ())?;
        let spec = match read_spec(&spec_path) {
            Ok(spec) => spec,
            Err(code) => {
                let _ = status.write_all(&[code]);
                return Err(());
            }
        };
        let _ = fs::remove_file(&spec_path);
        if let Err(code) = restrict(&spec) {
            let _ = status.write_all(&[code]);
            return Err(());
        }
        status.write_all(&[1]).map_err(|_| ())?;
        status.flush().map_err(|_| ())?;

        let (program, argv) = spec.argv.split_first().ok_or(())?;
        let mut command = Command::new(program);
        command
            .args(argv)
            .current_dir(&spec.cwd)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .envs(
                spec.environment
                    .iter()
                    .filter_map(|(key, value)| (key != "PATH").then_some((key, value))),
            );
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                let _ = status.write_all(&[0]);
                return Err(());
            }
        };
        status.write_all(&[1]).map_err(|_| ())?;
        status.flush().map_err(|_| ())?;
        let result = child.wait().map_err(|_| ())?;
        Ok(exit_code(result))
    }

    fn read_spec(path: &Path) -> Result<LaunchSpec, u8> {
        let metadata = fs::symlink_metadata(path).map_err(|_| 4u8)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || metadata.len() > MAX_SPEC_BYTES
        {
            return Err(5);
        }
        let file = fs::File::open(path).map_err(|_| 6u8)?;
        let opened = file.metadata().map_err(|_| 7u8)?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(8);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_SPEC_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| 9u8)?;
        if bytes.len() as u64 > MAX_SPEC_BYTES {
            return Err(10);
        }
        let spec: LaunchSpec = serde_json::from_slice(&bytes).map_err(|_| 11u8)?;
        if spec.schema_version != 1
            || spec.profile != "workspace_rw"
            || spec.argv.is_empty()
            || spec.argv.len() > MAX_ARG_COUNT
            || spec
                .argv
                .iter()
                .any(|arg| arg.is_empty() || arg.len() > MAX_ARG_BYTES || arg.contains('\0'))
            || spec.environment.len() > MAX_ENV_COUNT
            || spec.environment.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > MAX_ENV_NAME_BYTES
                    || value.len() > MAX_ENV_VALUE_BYTES
                    || key.contains('=')
                    || key.contains('\0')
                    || value.contains('\0')
                    || denied_environment_key(key) && key != "PATH"
                    || key == "PATH" && value != "/usr/bin:/bin"
            })
            || spec
                .environment
                .iter()
                .map(|(key, _)| key)
                .collect::<HashSet<_>>()
                .len()
                != spec.environment.len()
        {
            return Err(12);
        }
        let root = spec.root.canonicalize().map_err(|_| 13u8)?;
        let cwd = spec.cwd.canonicalize().map_err(|_| 14u8)?;
        if root != spec.root || !root.is_dir() || !cwd.starts_with(&root) || !cwd.is_dir() {
            return Err(15);
        }
        Ok(spec)
    }

    fn restrict(spec: &LaunchSpec) -> Result<RestrictionStatus, u8> {
        let abi = ABI::V4;
        let handled = AccessFs::from_all(abi);
        let mut ruleset = Ruleset::default()
            .handle_access(handled)
            .map_err(|_| 2u8)?
            .create()
            .map_err(|_| 2u8)?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(
                PathFd::new(&spec.root).map_err(|_| 2u8)?,
                handled,
            ))
            .map_err(|_| 2u8)?;

        for path in ["/usr", "/etc/ld.so.cache", "/etc/ssl/certs"] {
            let path = Path::new(path);
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            let runtime_read = if canonical.is_dir() {
                AccessFs::from_read(abi) | AccessFs::Execute
            } else {
                landlock::make_bitflags!(AccessFs::{ReadFile})
            };
            ruleset = ruleset
                .add_rule(PathBeneath::new(
                    PathFd::new(canonical).map_err(|_| 2u8)?,
                    runtime_read,
                ))
                .map_err(|_| 2u8)?;
        }
        let status = ruleset.restrict_self().map_err(|_| 2u8)?;
        if status.ruleset != RulesetStatus::FullyEnforced {
            return Err(30);
        }
        if !status.no_new_privs {
            return Err(31);
        }
        Ok(status)
    }

    fn exit_code(status: ExitStatus) -> i32 {
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    }

    fn denied_environment_key(key: &str) -> bool {
        let upper = key.to_ascii_uppercase();
        [
            "LD_",
            "DYLD_",
            "PATH",
            "IFS",
            "SHELLOPTS",
            "CDPATH",
            "BASH_ENV",
            "ENV",
            "PYTHONPATH",
            "PYTHONHOME",
            "NODE_OPTIONS",
            "RUBYOPT",
            "PERL5OPT",
            "GIT_ASKPASS",
            "SSH_ASKPASS",
            "GIT_CONFIG",
            "GIT_CONFIG_",
            "AWS_",
            "AZURE_",
            "GOOGLE_",
            "SSH_AUTH_SOCK",
        ]
        .iter()
        .any(|denied| upper == *denied || upper.starts_with(denied))
    }
}

#[cfg(target_os = "linux")]
fn main() {
    linux::entry();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    std::process::exit(125);
}
