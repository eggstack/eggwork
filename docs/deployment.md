# Deployment and service lifecycle

`eggworkd` defines the node's service identity and update policy. Eggup performs
installation transactions and service-manager operations. Before changing a
service, Eggup checks that its executable, config path, and critical arguments
match the expected Eggwork service. A service that does not match is left
untouched.

## Install a service

Every service command needs an executable identity and the absolute config
path the service will use. `--service-config` is required. `--executable`
defaults to the running `eggworkd`; pass it explicitly in scripts.

Inspect the service specification before installing it:

```bash
eggworkd service spec --config /etc/eggwork/node.json \
  --executable /opt/eggwork/bin/eggworkd \
  --service-config /etc/eggwork/node.json
```

Choose the platform policy for every service mutation:

| Platform | Required options | Notes |
|---|---|---|
| Linux | `--unit-path <absolute-path>` | `--scope system` or `--scope user`; defaults to `system`. |
| macOS | `--plist-path <absolute-path>` and `--launchd-domain user\|system` | For `user`, also pass `--launchd-target gui/<uid>`. For `system`, the target defaults to `system`. |
| Windows | None | `--windows-start-type automatic\|manual\|disabled` is optional and defaults to `manual`. |

For example, install a system-scoped Linux service:

```bash
eggworkd service install --config /etc/eggwork/node.json \
  --executable /opt/eggwork/bin/eggworkd \
  --service-config /etc/eggwork/node.json \
  --unit-path /etc/systemd/system/eggwork-node.service --scope system
```

Use the same identity and platform options for `status`, `start`, `stop`,
`restart`, and `uninstall`. The command reports JSON; inspect its `completed`
field and exit status to confirm a mutation succeeded.

On Windows, the service control manager starts
`eggworkd service-host --config <absolute-path>`. Source builds in this tree
include the service host. A published binary supports this mode only if that
version includes it. Service stop cancels active executions and waits for them
to finish. It does not clear an existing persistent drain marker.

## Update installed binaries

`deployment apply` replaces binaries from local candidate files. It does not
download a release or select one. `--release` is a label recorded in the
receipt, not a lookup key.

On Linux, provide both the daemon and sandbox-helper candidates. macOS and
Windows use the daemon only. Candidates must be absolute paths to regular
files.

```bash
OLD_DAEMON_SHA256=$(sha256sum /opt/eggwork/bin/eggworkd | cut -d' ' -f1)
OLD_HELPER_SHA256=$(sha256sum /opt/eggwork/bin/eggwork-sandbox-helper | cut -d' ' -f1)

eggworkd deployment apply --config /etc/eggwork/node.json \
  --installation-root /opt/eggwork \
  --release vX.Y.Z \
  --daemon /var/tmp/eggworkd \
  --helper /var/tmp/eggwork-sandbox-helper \
  --previous-daemon-sha256 "$OLD_DAEMON_SHA256" \
  --previous-helper-sha256 "$OLD_HELPER_SHA256" \
  --service-config /etc/eggwork/node.json \
  --unit-path /etc/systemd/system/eggwork-node.service --scope system
```

When replacing an existing install, provide the SHA-256 of each current
binary with `--previous-daemon-sha256` and, on Linux,
`--previous-helper-sha256`. Missing or incorrect proof fails closed.

After validating the candidates, `deployment apply` sets persistent drain
before waiting for executions and replacing files. It leaves the drain marker
set after success. If a later step fails, the marker remains set so an operator
can inspect or recover the node before it accepts work again. Check the JSON
receipt and service state, then run `eggworkd undrain` when the node is ready.

For a manually controlled update, the lifecycle is:

1. Prepare and validate the candidate binaries.
2. Run `deployment apply` and review its receipt.
3. Confirm the service and installed version are healthy.
4. Run `eggworkd undrain` to resume admission.

See [node operations](operations.md) for drain, readiness, and status commands.
