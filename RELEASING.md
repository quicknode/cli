# Releasing `qn`

How to cut a release. The pipeline is mostly automated via cargo-dist; a few channels still need manual maintainer steps until CI has the right credentials to do them itself.

## Per-release flow

Three named recipes in the `Justfile`:

1. **`just release-prepare X.Y.Z`** — orchestrates the bump → branch → PR → squash-merge → tag-push → wait-for-CI sequence. Tag push is what fires `release.yml`; do not also run `release-create-tag` (it races cargo-dist's host job).

   Internally calls `release-bump`, `release-open-pr`, `release-merge-pr`, `release-tag-main`, `release-wait-ci`. Each is also runnable standalone if a step fails and you need to retry.

2. **`release.yml` runs in CI** — cross-compiles 7 targets, creates the GitHub Release, attaches archives + sha256 sidecars + SLSA attestations, then fans out to per-channel publish jobs.

   Publish channels currently in CI:
   - `custom-publish-crates` → publishes `quicknode-cli` to crates.io
   - `custom-publish-docker` → builds multi-arch image, pushes to `ghcr.io/quicknode/qn`
   - `custom-publish-deb` → packages `.deb` per arch, uploads to the GitHub Release as assets

3. **Maintainer manual steps after CI succeeds.** Three channels still need a person to drive the publish because CI doesn't yet have the credentials it needs.

## Manual steps after each release

Run these from the repo root (`~/qn/cli`) after `release.yml` is green for the new tag.

### Homebrew

Sync the formula cargo-dist generated as a release artifact into the tap repo:

```fish
just release-update-homebrew-tap X.Y.Z ~/qn/homebrew-tap
git -C ~/qn/homebrew-tap push
```

The recipe downloads `qn.rb` from the GitHub Release, copies it to `Formula/qn.rb` in the tap clone, commits with a clean message, and prints the push command. It does not push itself — review the diff first.

### Scoop

Bump the canonical `version` in `bucket/qn.json`:

```fish
just release-update-scoop-bucket X.Y.Z ~/qn/scoop-bucket
git -C ~/qn/scoop-bucket push
```

The recipe pulls the Windows zip's sha256 from the release, renders a manifest with `version`, `hash`, and an `autoupdate` block, and stages it at `bucket/qn.json`. Once a user has tapped the bucket, `scoop update` finds new versions on its own — this manual step just keeps `scoop search qn` honest about what's current.

### AUR

Bump `pkgver` in the `qn-bin` AUR package:

```fish
just release-update-aur-bin X.Y.Z ~/qn/qn-bin
git -C ~/qn/qn-bin push
```

The recipe pulls both Linux gnu sha256 sidecars (x86_64 + aarch64) from the release, renders a `PKGBUILD` + `.SRCINFO`, and stages them. Push goes to `ssh://aur@aur.archlinux.org/qn-bin.git` — the AUR's git remote.

## One-time setup notes

A few channels needed manual setup the first time. Captured here so the next maintainer doesn't have to rediscover them.

### Homebrew tap (`quicknode/homebrew-tap`)

Public repo on GitHub. Must be public — `brew tap` does an anonymous git clone. Has a single `Formula/qn.rb` per formula. cargo-dist generates the formula as a release artifact (whether or not we auto-publish), so the maintainer's job is just to commit it into the tap.

### Scoop bucket (`quicknode/scoop-bucket`)

Public repo on GitHub. Must be public — Scoop does an anonymous git clone. Has `bucket/qn.json` per package. We hand-render the manifest in the recipe (cargo-dist doesn't generate one).

### AUR (`qn-bin`)

Maintainer needs an AUR account at <https://aur.archlinux.org> with an SSH key registered. Once that's set up:

```fish
# Confirm the name isn't taken (one-time, before first push)
curl -sf "https://aur.archlinux.org/rpc/v5/info?arg[]=qn-bin" \
  | python3 -c "import json,sys; print(json.load(sys.stdin).get('resultcount'))"
# 0 = free

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
