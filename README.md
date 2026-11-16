# Eggwork

Eggwork runs bounded commands on one explicitly selected remote node. Callers
choose the node and own queues, scheduling, and retries. A node authenticates
each caller with mutual TLS, reports execution events, and returns a terminal
result with any declared artifacts.

## Get started

Build a Linux node from source:

```bash
cargo build --locked -p eggwork-server --bin eggworkd
cargo build --locked -p eggwork-sandbox-helper
```

The [quickstart](docs/quickstart.md) creates the TLS material and node config,
checks readiness, starts the node, and verifies execution behavior.

## Connect a caller

`eggworkd` operates a node. Rust applications submit work through the
[`eggwork-client`](crates/eggwork-client) library, using `NodeClient` to connect
to one explicit node over mutual TLS and stream its execution events. Request
types live in [`eggwork-core`](crates/eggwork-core); there is no separate
submission CLI.

## Platform support

| Platform | Release binaries | Isolation notes |
|---|---|---|
| Linux x86_64, ARM64 | Daemon and sandbox helper | Runtime checks determine which Landlock and cgroup features are advertised. |
| macOS x86_64, ARM64 | Daemon | No Linux Landlock helper. |
| Windows x86_64 | Daemon | No filesystem or resource isolation. Declared outputs cannot be captured. Required isolation or resource requests fail before execution. |

The quickstart needs Linux, Rust through `rustup`, OpenSSL, and Python 3. See
[releases](docs/releases.md) for release availability and
[deployment](docs/deployment.md) for service installation and updates.

## Documentation

- [Quickstart](docs/quickstart.md): build and run a source-built Linux node.
- [Node operations](docs/operations.md): readiness, drain, inspection, and garbage collection.
- [Deployment](docs/deployment.md): service lifecycle and local binary updates.
- [Releases](docs/releases.md): supported targets and published release installation.
