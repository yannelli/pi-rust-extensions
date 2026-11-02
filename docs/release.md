# Release

Created 2026-10-29. Last updated 2026-11-02.

## Start a release

Releases are automatic, as in pi-rust. After CI passes on `main`, `.github/workflows/release.yml` runs `cargo xtask plan`. The plan reads the conventional commits since the last `v*` tag:

| Commit | Bump before 1.0 | Bump from 1.0 |
| --- | --- | --- |
| `feat!:`, `fix!:`, or a `BREAKING CHANGE:` line | Minor | Major |
| `feat:` | Patch | Minor |
| `fix:`, `revert:`, `perf:` | Patch | Patch |
| `chore:`, `ci:`, `docs:`, `test:`, `refactor:`, and others | None | None |

When a release is due, the `plan` job runs `cargo xtask prepare <version>`. It sets the workspace version in `Cargo.toml` and each extension version in `marketplace.json`, refreshes `Cargo.lock`, and adds a section to `CHANGELOG.md`. It commits `chore(release): v<version>` to `main`, pushes the annotated tag, and builds and publishes in the same run. A push with `GITHUB_TOKEN` starts no other workflow, so the tag does not start a second run. When `main` has moved past the commit CI tested, the run skips publishing and the next CI run on `main` releases it.

A workspace version without a tag is released as is. To pick a version by hand, set `version` in `[workspace.package]` and update `marketplace.json`, then merge. Pushing the tag `v<version>` yourself also releases that exact version, and `cargo xtask check-tag` stops the run when the tag is not `v` plus the workspace version.

Run the workflow by hand with `bump` (`auto`, `patch`, `minor`, or `major`) and `dry_run`. A dry run builds every target and the index without a tag or a release. A tag such as `v0.2.0-rc.1` creates a GitHub prerelease. `releases/latest` excludes prereleases.

## What the workflow does

The workflow builds `pi-rust-ext` for these targets:

| Target | Build tool |
| --- | --- |
| `aarch64-apple-darwin` | Cargo |
| `x86_64-apple-darwin` | Cargo |
| `x86_64-unknown-linux-musl` | cargo-zigbuild |
| `aarch64-unknown-linux-musl` | cargo-zigbuild |
| `x86_64-pc-windows-msvc` | Cargo |

Each build becomes `pi-rust-ext-<target>.tar.gz`, or `.zip` for Windows.
The archive root holds the binary, `LICENSE`, and `README.md`.
The binary is `pi-rust-ext`, or `pi-rust-ext.exe` on Windows.

The workflow runs `cargo test --workspace --locked` before it builds the archives.
Before the tests, `scripts/fetch-ci-pi.py` installs the published pi host v0.9.2 for the runner, and the tests run with `PI_REQUIRE_E2E=1` (see [Real-pi tests](#real-pi-tests)).
Linux builds use `cargo-zigbuild` 0.24.0 and Zig 0.15.2.
The `index` job writes `SHA256SUMS` with `sha256sum`. It then runs `marketplace-index` to write `index.json`.

The `sign` job uses the `release` environment, whose deployment policy permits `v*` tags and the `main` branch (automatic runs deploy from `main`).
Its `MARKETPLACE_SIGNING_KEY` secret holds an Ed25519 key in PKCS#8 PEM format.
The job downloads the index artifact and signs the exact bytes with system OpenSSL.
It verifies the signature against the client public key, then uploads `index.json.sig` as an artifact.
The signature is base64 on one line.
This job has no checkout or Rust build steps.

The `publish` job downloads the archives, index, and signature.
It runs `scripts/verify-release.sh <tag> dist` and creates provenance attestations for the archives and `index.json`.
It then creates a GitHub release with the `CHANGELOG.md` section as notes (generated notes when there is none) and uploads the archives, index, and signature. When the release exists, it uploads the files to it.

To check an attestation, run `gh attestation verify <file> -R yannelli/pi-rust-extensions`.

## marketplace-index

```sh
cargo run -p marketplace-index -- marketplace.json <tag> <archive-dir> <index.json>
```

Inputs:

- `marketplace.json`: the catalog described below.
- `<tag>`: must be `v` plus the workspace version.
- `<archive-dir>`: must contain the 5 archives and a `SHA256SUMS` file in `sha256sum` format.

The generator reads digests from `SHA256SUMS` and sizes from the archive files.

| Catalog field | Value |
| --- | --- |
| `schemaVersion` | Must be 1 |
| `protocolVersion` | Default: 1 |
| `minPiVersion` | Default: `0.7.7` |
| `extensions` | Array of extension entries |

Each extension needs `name`, `displayName`, and `description`.
The optional `version` defaults to the workspace version. The optional `args` defaults to `[name]`; its first item must be the name.
The optional `tags` field holds an array of strings.
The generator sets `binary` and `platforms`. It rejects unknown fields.

Validation:

- Extension names: 1 to 64 characters of `a-z`, `0-9`, and `-`, with no `-` at the start or end.
- Versions: SemVer 2.0 with an optional prerelease and no leading zeros in numeric parts. The generator rejects build metadata (`+...`).

## Verify a release

```sh
scripts/verify-release.sh v0.1.0
```

The script downloads `index.json` and `index.json.sig`. It checks the signature against the public key.
It checks `protocolVersion` is 1 and `minPiVersion` is a SemVer string without build metadata.
It then downloads each archive and checks its SHA-256 and size.
It needs OpenSSL 3, `jq`, `shasum`, and `curl`.
The macOS system `openssl` is LibreSSL. On macOS, use Homebrew `openssl@3`.

To check local artifacts, pass the directory as the second argument.
For test keys, use `--public-key <base64-raw-key>` before the tag.
`bash scripts/test-verify-release.sh` generates a local release and a temporary key to test verification failures.
CI also loads `marketplace.json` through the index generator to validate its entries.

## Real-pi tests

Tests that start pi call `pi_ext_e2e::pi_bin_or_skip`:

| Environment | Result |
| --- | --- |
| `PI_REQUIRE_E2E=1` without `PI_BIN` | The test fails |
| `PI_BIN` set to a path that is not an executable file | The test fails, also locally |
| `PI_BIN` set to an executable file | The test runs that pi |
| Neither set, and the default `pi-dev` path is missing | The test prints `skipping: pi binary ...` and passes |

CI (`check` on Ubuntu and macOS) and the release `test` job do the following:

1. `python3 scripts/fetch-ci-pi.py --dest "$RUNNER_TEMP/pi-host"` takes the `yannelli/pi-rust` release `v0.9.2` asset for the runner (`aarch64-apple-darwin`, `x86_64-apple-darwin`, or the Linux musl builds).
2. It checks the asset against the pinned SHA-256, the release API digest, and `SHA256SUMS`, and extracts only the regular file `pi`.
3. It runs `pi --version`, then appends `PI_BIN` and `PI_HOST_SHA256` to `$GITHUB_ENV`.
4. The workspace tests then run with `PI_REQUIRE_E2E=1`.

To change the host release, update `TAG` and `PINNED` in the script from a verified release.

The CI job `lens-exit` (Ubuntu and macOS) runs `python3 scripts/lens_probe/check.py --pi "$PI_BIN" --pi-sha256 "$PI_HOST_SHA256" --marketplace-tag v0.3.0 --out <new dir>`:

1. It installs the published `marketplace:lens@v0.3.0` into isolated directories.
2. It drives the pi TUI against a loopback mock model, and starts a stalled LSP through a `write app.py` turn.
3. It quits with Ctrl+D and then with SIGTERM.
4. It passes when both quits exit 0 within 12 s and the Lens process (the installed `pi-rust-ext lens`), the LSP, and its child are gone 3 s later.

The job uploads `report.json` and the screen captures, also on failure.

## Action versions

The workflows pin these action versions by commit SHA, resolved from GitHub tags on 2026-10-30:

| Action | Version |
| --- | --- |
| `actions/checkout` | v7.0.1 |
| `actions/upload-artifact` | v7.0.2 |
| `actions/download-artifact` | v8.0.2 |
| `actions/attest-build-provenance` | v4.2.2 |
| `Swatinem/rust-cache` | v2.9.3 |

The workflows use the runner's `rustup` to install stable Rust.

`attest-build-provenance` wraps `actions/attest`. It needs `id-token: write` and `attestations: write`.

Sources: <https://github.com/actions/attest-build-provenance>, <https://github.com/actions/attest>, <https://github.com/actions/checkout/releases>.
