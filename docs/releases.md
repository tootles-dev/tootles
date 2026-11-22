# Signed CLI releases

The [GitHub release workflow](../.github/workflows/release.yml) builds standalone Tootles executables and signs the final download archives. It runs on GitHub; Forgejo uses its separate CI workflow.

## Platforms and contents

| Platform | Target | Build runner |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | Ubuntu 24.04 |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | Ubuntu 24.04 ARM64 |
| Apple Silicon macOS | `aarch64-apple-darwin` | macOS 15 |

Linux archives require Ubuntu 24.04 or a compatible newer system with glibc. Build from source on older distributions or musl-based systems. macOS builds and tests run natively in the release workflow; interactive terminal behavior still needs testing on your Mac. Native Windows and Intel Mac packages are not currently produced.

Each versioned archive, such as `tootles-v0.2.0-x86_64-unknown-linux-gnu.tar.gz`, contains the executable, README, license, man page, desktop entry, docs, and Bash, Zsh, and Fish completions. Its accompanying files are:

| File suffix | Purpose |
| --- | --- |
| `.tar.gz.sha256` | SHA-256 checksum for the archive |
| `.tar.gz.sigstore.json` | Cosign signature, signing certificate, and transparency-log verification material |
| `.tar.gz.intoto.jsonl` | Signed GitHub build provenance bundle |

The archive signatures use [Sigstore keyless signing](https://docs.sigstore.dev/quickstart/quickstart-ci/) with GitHub Actions OIDC. They identify the GitHub repository, release workflow, and tag. [GitHub build provenance](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations) also records the source revision and build workflow. No long-lived private signing key or signing-key secret is required. Signing certificates and transparency-log records are public.

These are signatures for verifying downloads. The macOS executable is not Apple Developer ID signed or notarized; that would require separate Apple credentials and a notarization step. Checksums alone do not authenticate a download.

## Verify and install a download

Install [Cosign 3](https://docs.sigstore.dev/cosign/system_config/installation/) and, for provenance verification, a current [GitHub CLI](https://cli.github.com/). Use the public GitHub mirror's actual `OWNER/REPO` below; the Forgejo repository address is not a GitHub signing identity. Select a published version and the target from the table above.

```sh
repo='OWNER/REPO'
version='0.2.0'
tag="v$version"
target='x86_64-unknown-linux-gnu'
archive="tootles-$tag-$target.tar.gz"
identity="https://github.com/$repo/.github/workflows/release.yml@refs/tags/$tag"
```

Download the archive and its three accompanying files from that GitHub release page. Alternatively, after signing in with `gh auth login`:

```sh
gh release download "$tag" --repo "$repo" \
  --pattern "$archive" --pattern "$archive.sha256" \
  --pattern "$archive.sigstore.json" --pattern "$archive.intoto.jsonl"
```

Verify the signature against the **exact workflow and tag**, then verify build provenance:

```sh
cosign verify-blob "$archive" --bundle "$archive.sigstore.json" \
  --certificate-identity "$identity" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com

gh attestation verify "$archive" --repo "$repo" \
  --cert-identity "$identity" --source-ref "refs/tags/$tag" \
  --deny-self-hosted-runners

shasum -a 256 --check "$archive.sha256"
```

The GitHub command looks up attestations from the repository. To verify using the downloaded provenance bundle instead, add `--bundle "$archive.intoto.jsonl"`. To require a specific reviewed source commit, also add `--source-digest FULL_COMMIT_SHA`. Cosign and GitHub CLI may fetch current trust roots; fully offline verification requires preparing trusted roots separately. See the [GitHub verification options](https://cli.github.com/manual/gh_attestation_verify). Do not replace the exact certificate identity with a wildcard.

Continue only after verification succeeds:

```sh
tar -xzf "$archive"
mkdir -p "$HOME/.local/bin"
install -m 755 "${archive%.tar.gz}/tootles" "$HOME/.local/bin/tootles"
"$HOME/.local/bin/tootles" --version
```

Add `~/.local/bin` to `PATH`, then run `tootles --demo`. Rust and Cargo are not needed to run this binary. See the [installation requirements](../README.md#requirements) for terminal, image, and optional video support.

## Configure the public GitHub mirror

1. Sync this source, including `.github/workflows`, to the public GitHub repository. Enable GitHub Actions and allow the actions referenced by the workflows. No hardcoded repository name needs changing.
2. Allow the workflow's job permissions: `contents: read`, `id-token: write`, and `attestations: write` for building and signing, and `contents: write` for publishing. The automatically supplied `GITHUB_TOKEN` is sufficient; no PAT, Cosign key, or Apple certificate is needed in repository secrets. Organization policies can restrict these permissions or external actions.
3. Sync release tags as well as commits. A tag must exactly match the package version in `Cargo.toml`, for example `v0.2.0` for version `0.2.0`. Ensure the tag includes this workflow. Restrict release tag creation to maintainers using repository rules.

GitHub artifact attestations are available for public repositories on current GitHub plans. Private mirrors on Free, Pro, or Team do not have the same attestation support. If mirroring is performed by another GitHub workflow, pushes using its default `GITHUB_TOKEN` do not trigger new workflow runs; use an appropriately scoped GitHub App or PAT for that mirror operation, or manually dispatch this release workflow on the synced tag. That mirror credential is separate from release signing. See [triggering workflows](https://docs.github.com/en/actions/how-tos/writing-workflows/choosing-when-your-workflow-runs/triggering-a-workflow).

## Build and publish

The **Release CLI** workflow offers two entry points:

- Push a matching `v*` tag to GitHub, or manually run the workflow on that tag: build, test, sign, verify, and publish a release. Versions containing a prerelease suffix are marked as prereleases.
- Manually run the workflow on a branch: build and test all platforms, then download the unsigned archives and checksums from the run's artifacts. It does not publish a release or request signing certificates.

Every platform runs formatting, Clippy, and Rust tests before compiling its release executable. Builds use the lockfile, explicit native targets, and fresh build output. Release actions are pinned to commit SHAs. Each completed archive is signed with Cosign and attested by GitHub, then the publishing job verifies all three archives, checksums, signatures, and provenance against the same tag and source commit. No archive is published if any platform or verification step fails.

The workflow uploads release assets to a new draft and publishes it only after all uploads succeed. It refuses to overwrite an existing release. If an upload or final publication fails, inspect the draft before recovery: either publish it after confirming all verified assets are present, or delete only the incomplete draft (keep its Git tag) and rerun the failed publishing job. For a new build of already published code, use a new version and tag rather than replacing existing assets.

Local validation can check workflow syntax, packaging, and verification commands. The first GitHub tag run is still needed to exercise GitHub's OIDC signing and native hosted runners end to end; merely installing Tootles or pushing a branch does not create a signed release.
