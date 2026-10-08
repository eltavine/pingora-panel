# 0046: Supply-chain evidence

Status: accepted. Builds on [ADR 0029](0029-gateway-allocator-and-release-builds.md).

## Context

CI already checks Rust advisories and licenses with cargo-deny, scans the
source with Semgrep and the history with Gitleaks, and holds every exception
to an owner, a reason and an expiry. What an operator receives has none of
that evidence: there is no list of what an image or a release contains, no
scan of the console's dependencies or of the image, no notice of the
third-party licenses shipped, and nothing proves who built an image or a
binary, or from which commit. The specification asks for SBOMs of the Rust,
Node and container dependencies, signatures on release artifacts, images,
SBOMs and provenance, scans of dependencies and containers, and that no
Critical or High finding is left unexplained.

## Decision

**SBOMs.** Syft writes CycloneDX JSON for the Rust workspace from
`Cargo.lock`, for the console from `pnpm-lock.yaml` and for each image.
Binaries are built with cargo-auditable, which embeds the crates they were
compiled from, so the image's SBOM lists the Rust dependencies in it and not
only its operating system packages. CI keeps the SBOMs of every build as
artifacts; a release attaches them.

**Scans.** Grype scans the console's and the image's SBOMs and fails on a
Critical or High vulnerability unless a lease in
`.github/policies/vulnerability-leases.json` names it, with an owner, a
reason and an expiry, as the other exceptions are held; a lease that matches
nothing fails too, so none outlives its finding. Rust crates stay with
cargo-deny and the RustSec database: the image scan leaves them to it rather
than holding the same advisory under two identifiers.

**Licenses.** cargo-about writes the licenses of the crates compiled in, with
their texts, accepting the licenses cargo-deny allows, and pnpm lists the
console's production dependencies' licenses. The two make one notice that the
image carries under `/usr/share/doc/pingora-panel/` and a release attaches.

**Signatures and provenance.** A release is built by one workflow from a
version tag: native amd64 and arm64 builds, pushed to GHCR by digest and
joined into one index. The index is signed with cosign without keys: Fulcio
binds the signature to the workflow's identity and Rekor records it.
GitHub's artifact attestations add SLSA build provenance for the image and
for every release asset, and an SBOM attestation for the image; each asset
also gets a Sigstore bundle, so it can be checked without GitHub:

```sh
cosign verify ghcr.io/eltavine/pingora-panel@sha256:<digest> \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github.com/eltavine/pingora-panel/\.github/workflows/release\.yml@refs/tags/v'
gh attestation verify oci://ghcr.io/eltavine/pingora-panel@sha256:<digest> \
  --repo eltavine/pingora-panel
cosign verify-blob ppanel-x86_64-unknown-linux-gnu.tar.gz \
  --bundle ppanel-x86_64-unknown-linux-gnu.tar.gz.sigstore.json \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github.com/eltavine/pingora-panel/\.github/workflows/release\.yml@refs/tags/v'
```

Tools are installed at fixed versions whose archives are checked against
their published digests, as the existing installers are.

## Alternatives

- Long-lived signing keys: keys to guard, rotate and revoke; keyless
  signatures bind to the workflow and expire with their certificate.
- `slsa-github-generator`: equivalent provenance through reusable workflows;
  artifact attestations are GitHub's own and verify with `gh`.
- Trivy: an equally capable scanner; Syft and Grype share one SBOM, so what is
  scanned is exactly what is published.
- `cargo cyclonedx`: richer Rust metadata, but only for Rust, while the image
  and the console need an SBOM too.

## Consequences

- Images are published to GHCR, and an installation can pin a digest and
  verify it before running it.
- A new Critical or High finding fails CI until it is fixed or leased.
- Release assets are reproducible from their provenance: the workflow, the
  commit and the inputs are recorded and signed.
