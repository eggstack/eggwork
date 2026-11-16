# Releases

Eggwork releases are published as exact version tags. There are no published
releases currently. Release builds staged by CI remain drafts until a
maintainer publishes them; draft assets are not public downloads.

## Supported targets

| Target | Release contents |
|---|---|
| `x86_64-unknown-linux-gnu` | `eggworkd` and `eggwork-sandbox-helper` |
| `aarch64-unknown-linux-gnu` | `eggworkd` and `eggwork-sandbox-helper` |
| `x86_64-apple-darwin` | `eggworkd` |
| `aarch64-apple-darwin` | `eggworkd` |
| `x86_64-pc-windows-msvc` | `eggworkd.exe` |

Linux ships the daemon and helper together so their versions stay matched. The
helper provides Landlock workspace isolation on supported Linux hosts.

## First install

After a release is published, use its exact tag. The release's `install.sh` or
`install.ps1` installs the matching binaries into a destination directory. The
scripts are for first install only: they do not overwrite existing binaries,
create node configuration, elevate privileges, or register a service.

For Linux or macOS:

```bash
TAG=vX.Y.Z
curl -fSLO "https://github.com/eggstack/eggwork/releases/download/$TAG/install.sh"
less install.sh
sh install.sh /opt/eggwork/bin
```

On Windows, download and review `install.ps1` from the same release, then run
it with PowerShell:

```powershell
$tag = "vX.Y.Z"
Invoke-WebRequest "https://github.com/eggstack/eggwork/releases/download/$tag/install.ps1" -OutFile install.ps1
Get-Content .\install.ps1
pwsh -File .\install.ps1 C:\Eggwork\bin
```

Use only assets for your platform and CPU architecture.

Each release artifact has a `.sha256` sidecar. Verify it when downloading
artifacts directly. SHA-256 checks detect changes to the bytes, but do not
prove who published them.

After installation, create a node config and TLS material using the
[example config](../examples/eggworkd.example.json) and the
[quickstart](quickstart.md). The quickstart commands are for Linux; on macOS
and Windows, omit `sandbox_helper` and adapt paths and file permissions. Run
`eggworkd version` to confirm the installed binary, then validate the config
with `eggworkd doctor --config <node.json>`.

## Updates

The bootstrap scripts do not update an existing installation. Use
[`deployment apply`](deployment.md#update-installed-binaries) with candidate
binaries acquired from a trusted release and verify the receipt before
undraining the node.
