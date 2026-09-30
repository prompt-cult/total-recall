# Installation

Three routes, in order of how much they assume: a release binary fetched by a
version manager, a release binary you download by hand, or a build from
source. All three produce the same executable, `total-recall`, at the archive
root.

## mise

```bash
mise use github:prompt-cult/total-recall
```

mise selects the archive for your OS and CPU by reading the triple out of the
asset name, so the same line installs the right binary on macOS arm64, macOS
x86_64, Linux x86_64 and Linux arm64. It also fetches the release's
`SHA256SUMS` and verifies the download against it.

> The older `mise use ubi:prompt-cult/total-recall` spelling still resolves,
> but mise has deprecated the ubi backend in favour of the `github:` backend.

## aqua

aqua resolves assets through a registry entry rather than by inspecting
release names, so it needs a `registry.yaml` mapping this project's assets. The
asset layout below is what such an entry targets:

```yaml
packages:
  - type: github_release
    repo_owner: prompt-cult
    repo_name: total-recall
    asset: 'total-recall_{{trimV .Version}}_{{.Arch}}-{{.OS}}.tar.gz'
    format: tar.gz
    replacements:
      amd64: x86_64
      arm64: aarch64
      darwin: apple-darwin
      linux: unknown-linux-gnu
```

`aqua g -i prompt-cult/total-recall` works once that entry is in a registry
aqua is configured to read.

## Plain download

Prebuilt archives ship on every tagged release. Asset names are
`total-recall_<version>_<target-triple>.tar.gz` — for example
`total-recall_0.7.0_x86_64-unknown-linux-gnu.tar.gz` — and each archive holds
one executable, `total-recall`, at its root.

```bash
VERSION=0.7.0
curl -fsSLO "https://github.com/prompt-cult/total-recall/releases/download/v${VERSION}/total-recall_${VERSION}_x86_64-unknown-linux-gnu.tar.gz"
curl -fsSLO "https://github.com/prompt-cult/total-recall/releases/download/v${VERSION}/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "total-recall_${VERSION}_x86_64-unknown-linux-gnu.tar.gz"
install -m 0755 total-recall /usr/local/bin/
```

Substitute the triple for your platform: `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, or
`aarch64-unknown-linux-gnu`.

### Verifying a download

Every release carries a `SHA256SUMS` over all four archives, one line each,
`<64 hex digits>` then two spaces then the bare asset filename. `sha256sum
--check SHA256SUMS` verifies them all; `--ignore-missing` verifies only the
archives you downloaded. mise reads the same file automatically. The archives
are built reproducibly, so a given source tree always yields the same digest.

## Build from source

Requires a stable Rust toolchain. The default build compiles both LLM vendors
and needs an Inception API key for the LLM-backed tools:

```bash
cargo build --release
./target/release/total-recall --version
```

For a vendor-free build — no API key, no LLM calls, every log-mining tool
working — see [Configuration](configuration.md#build-features):

```bash
cargo build --release --no-default-features
```

## API key

The key is only needed by the LLM-backed tools (`compact_session` / `compact`,
`total_recall` / `recall`). Get one from
[https://platform.inceptionlabs.ai](https://platform.inceptionlabs.ai), then
either:

- copy `.env.template` to `.env` and fill it in — the build-from-source CLI
  path, since the CLI reads `.env` relative to its working directory; or
- put the key in the MCP registration snippet's environment block, which is
  what an MCP server actually reads: see [MCP setup](mcp-setup.md).

A `.env` file is only loaded by a default build, and only relative to the
process's working directory. A spawned MCP server's working directory is
chosen by the client, not by you, so the environment block is the reliable
route for a server.

For `--provider mistral`, set `MISTRAL_API_KEY` instead.

## Requirements

- Rust toolchain (stable), for building from source
- `INCEPTION_API_KEY` in the environment or `.env`, for the LLM-backed tools on a default build
- Optional: `MISTRAL_API_KEY`, for `--provider mistral`
- `sqlite3` on `PATH` only for the [store-vacuum runbook](operations/opencode-store-vacuum.md)

Licensed `MIT OR Apache-2.0`.
