# Releasing `qn`

How to cut a release. The pipeline is mostly automated via cargo-dist; a few channels still need manual maintainer steps until CI has the right credentials to do them itself.

## Quick release

If the one-time setup below is done (tap, bucket, and AUR clones live under `~/qn/`), a full release is two commands:

```fish
just release-prepare X.Y.Z
# release-prepare drives bump → PR → squash-merge → tag → CI through to a green run.
# Watch the workflow; come back when it's done.

just release-finalize
# Auto-detects the just-released version from the latest git tag, then bumps
# the Homebrew tap, Scoop bucket, and AUR qn-bin clones to match. Opens a PR
# against the tap and bucket repos (their main branches are protected) and
# prints the `git push` command for AUR. Merge the two PRs and run the push
# to publish.
```

Override the clone-root directory if your clones live elsewhere: `just release-finalize ~/work/quicknode`. Override the version too if you're backfilling an older release: `just release-finalize ~/qn 0.1.4`.

The rest of this document covers what each step does in detail, what to do if part of the pipeline fails, and the one-time setup for each channel.

## Per-release flow

Three named recipes in the `Justfile`:

1. **`just release-prepare X.Y.Z`** — orchestrates the bump → branch → PR → squash-merge → tag-push → wait-for-CI sequence. Tag push is what fires `release.yml`; do not also run `release-create-tag` (it races cargo-dist's host job).

   Internally calls `release-bump`, `release-open-pr`, `release-merge-pr`, `release-tag-main`, `release-wait-ci`. Each is also runnable standalone if a step fails and you need to retry.

2. **`release.yml` runs in CI** — cross-compiles 7 targets, creates the GitHub Release, attaches archives + sha256 sidecars + SLSA attestations, then fans out to per-channel publish jobs.

   Publish channels currently in CI:
   - `custom-publish-crates` → publishes `quicknode-cli` to crates.io
   - `custom-publish-docker` → builds multi-arch image, pushes to `ghcr.io/quicknode/qn`
   - `custom-publish-deb` → packages `.deb` per arch, uploads to the GitHub Release as assets
   - `custom-publish-copr` → builds an SRPM from `packaging/qn-bin.spec` (whose `%prep` downloads the SLSA-attested prebuilt binary), dispatches via `copr-cli build` to the `quicknode/qn` COPR project

3. **Maintainer manual steps after CI succeeds.** Some channels still need a person to drive the publish because CI doesn't yet have the credentials it needs.

## Manual steps after each release

Run these from the repo root (`~/qn/cli`) after `release.yml` is green for the new tag.

### Homebrew

Sync the formula cargo-dist generated as a release artifact into the tap repo:

```fish
just release-update-homebrew-tap X.Y.Z ~/qn/homebrew-tap
```

The recipe downloads `qn.rb` from the GitHub Release, copies it to `Formula/qn.rb` on a `release/vX.Y.Z` branch cut from the tap's latest `origin/main`, commits, pushes the branch, and opens a PR (the tap's `main` is protected, so changes must land via PR). Review and merge the PR to publish. Re-running the recipe recreates the branch and updates the open PR.

### Scoop

Bump the canonical `version` in `bucket/qn.json`:

```fish
just release-update-scoop-bucket X.Y.Z ~/qn/scoop-bucket
```

The recipe pulls the Windows zip's sha256 from the release, renders a manifest with `version`, `hash`, and an `autoupdate` block at `bucket/qn.json`, commits it on a `release/vX.Y.Z` branch cut from the bucket's latest `origin/main`, pushes the branch, and opens a PR (the bucket's `main` is protected, so changes must land via PR). Review and merge the PR to publish. Once a user has tapped the bucket, `scoop update` finds new versions on its own — this manual step just keeps `scoop search qn` honest about what's current.

### AUR

Bump `pkgver` in the `qn-bin` AUR package:

```fish
just release-update-aur-bin X.Y.Z ~/qn/qn-bin
git -C ~/qn/qn-bin push
```

The recipe pulls both Linux gnu sha256 sidecars (x86_64 + aarch64) from the release, renders a `PKGBUILD` + `.SRCINFO`, and stages them. Push goes to `ssh://aur@aur.archlinux.org/qn-bin.git` — the AUR's git remote.

### Curated install block on the release notes

`release-finalize` finishes by calling `release-update-install-notes`, which prepends a curated "How to install" section to the GitHub release body. Source for the block is `packaging/release-notes-install.md.tmpl`; edit it there if the install copy needs to change. The recipe is idempotent — re-running against the same release replaces the existing block rather than stacking duplicates — so it's safe to invoke standalone:

```fish
just release-update-install-notes X.Y.Z            # edits the release in place
just release-update-install-notes X.Y.Z --dry-run  # prints the assembled body without editing
```

cargo-dist's auto-generated content is preserved below a `---` separator.

## One-time setup notes

A few channels needed manual setup the first time. Captured here so the next maintainer doesn't have to rediscover them.

### Homebrew tap (`quicknode/homebrew-tap`)

Public repo on GitHub. Must be public — `brew tap` does an anonymous git clone. Has a single `Formula/qn.rb` per formula. cargo-dist generates the formula as a release artifact (whether or not we auto-publish), so the maintainer's job is just to commit it into the tap.

### Scoop bucket (`quicknode/scoop-bucket`)

Public repo on GitHub. Must be public — Scoop does an anonymous git clone. Has `bucket/qn.json` per package. We hand-render the manifest in the recipe (cargo-dist doesn't generate one).

### AUR (`qn-bin`)

Maintainer needs an AUR account at <https://aur.archlinux.org> with an SSH key registered. Once that's set up:

```fish
# Clone the (currently empty) AUR git remote
mkdir -p ~/qn
cd ~/qn
git clone ssh://aur@aur.archlinux.org/qn-bin.git
# AUR returns: "warning: You appear to have cloned an empty repository." — expected.

# Render PKGBUILD + .SRCINFO from the latest release
cd ~/qn/cli
just release-update-aur-bin X.Y.Z ~/qn/qn-bin

# AUR expects the default branch to be `master`. Modern git defaults to `main`,
# so rename the freshly-created branch before the first push.
git -C ~/qn/qn-bin branch -m main master
git -C ~/qn/qn-bin push -u origin master
```

The first push registers the package on the AUR. Subsequent pushes just update it — no rename needed (the branch stays `master`).

After publishing: confirm via `https://aur.archlinux.org/packages/qn-bin` (the RPC at `/rpc/v5/info` can lag the package page by a few minutes — trust the web page, not the RPC, for fresh registrations).

### COPR (`quicknode/qn`)

Maintainer needs a Fedora account at <https://accounts.fedoraproject.org> and a COPR project `quicknode/qn` created at <https://copr.fedorainfracloud.org>. Project settings to set on creation:

- **Chroots**: current Fedora releases + EPEL 9 (covers RHEL 9, Rocky 9, Alma 9). Skip EPEL 8 unless requested — its glibc is too old for our gnu binaries.
- **Build settings**: enable internet access for builds (the `%prep` step in `packaging/qn-bin.spec` curls the prebuilt tarball from the GitHub Release; without net the build can't download it).

CI auths to COPR via four repo secrets — `copr-cli` itself only reads credentials from a config file at `~/.config/copr` (no env-var fallback), so we provision the four fields separately and let the workflow assemble the file at build time. Generate the values at <https://copr.fedorainfracloud.org/api/>; the page shows a `[copr-cli]` config block with these four lines. Copy each field's value into its own secret:

| Secret | Field from the COPR API page |
|---|---|
| `COPR_LOGIN`    | `login = …` |
| `COPR_USERNAME` | `username = …` |
| `COPR_TOKEN`    | `token = …` |
| `COPR_URL`      | `copr_url = …` (almost always `https://copr.fedorainfracloud.org`) |

Set each with `gh secret set COPR_LOGIN --repo quicknode/cli` etc., pasting just the value (no `login = ` prefix, no quotes). Splitting them this way also means the token can be rotated without re-pasting the other three.

`packaging/qn-bin.spec` lives in this repo; it's a thin spec whose `%prep` downloads the SLSA-attested prebuilt linux-gnu tarball and verifies it against the `.sha256` sidecar, so COPR isn't rebuilding `qn` from Rust source — it's just packaging the upstream binary into an RPM per chroot. Same trust chain as everywhere else qn ships.

## Recovery: a publish channel failed

If a single publish-* job in `release.yml` fails (e.g. crates.io rejected the publish because the token expired), the rest of the release is still good — the GitHub Release, attestations, and other channels remain published.

To retry just the failed job:

```fish
gh run rerun <run-id> --failed --repo quicknode/cli
```

For crates.io specifically, the manual fallback if CI's auth is broken is:

```fish
just release-cargo-publish
# Requires `cargo login` first.
```

## Sanity-checks before tagging

- `just lint` clean (`cargo clippy --all-targets -- -D warnings`)
- `just test` clean
- `just release-cargo-publish-check` clean (validates the crate tarball without uploading)
- `dist plan` exits 0 (verifies the generated workflow matches `dist-workspace.toml`)

If `dist plan` complains the workflow is out of date, run `just dist-regen` to regenerate and commit the result.
