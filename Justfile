# Local development

# Run the test suite.
test:
  cargo test --all

# Run clippy in deny-warnings mode against everything.
lint:
  cargo clippy --all-targets -- -D warnings

# Verify formatting without modifying anything.
fmt-check:
  cargo fmt --all -- --check

# Apply formatting.
fmt:
  cargo fmt --all

# Release pipeline maintenance

# Regenerate the dist workflow. Run this any time you change
# dist-workspace.toml or upgrade cargo-dist.
#
# Note: cargo-dist's `plan` step does an internal "is the generated
# workflow up to date" check and hard-fails CI on any divergence, so we
# can't post-process the output here. The "axo bot" attribution on tap
# repo commits stays as-is until upstream exposes a config knob.
dist-regen:
  dist generate

# Validate the crate tarball without uploading. Run before tagging to
# catch packaging errors (missing license, README, dirty tree, etc.)
# while there's still time to fix them.
release-cargo-publish-check:
  cargo publish -p quicknode-cli --dry-run

# Publish the crate to crates.io. Normally invoked from CI by the
# custom-publish-crates job in release.yml; this recipe is for manual
# recovery if CI's publish step fails and we need to retry from a
# clean local tree. Requires `cargo login` first.
release-cargo-publish:
  cargo publish -p quicknode-cli

# Manually update the Homebrew tap with the formula attached to a given
# release. Use this until CI has a PAT with contents:write on the tap
# repo and can automate the formula push — at which point we add
# "homebrew" to publish-jobs in dist-workspace.toml, the cargo-dist
# workflow takes over, and this recipe becomes a manual-recovery fallback.
#
# Usage: just release-update-homebrew-tap 0.1.0 ~/qn/homebrew-tap
#
# Precondition: tap_path is a clean local clone of quicknode/homebrew-tap.
release-update-homebrew-tap version tap_path:
  #!/usr/bin/env bash
  set -euo pipefail
  if [[ ! -d "{{tap_path}}/.git" ]]; then
    echo "Error: {{tap_path}} is not a git checkout. Clone quicknode/homebrew-tap there first." >&2
    exit 1
  fi
  if ! git -C "{{tap_path}}" diff --quiet || ! git -C "{{tap_path}}" diff --cached --quiet; then
    echo "Error: {{tap_path}} has uncommitted changes. Commit or stash them first." >&2
    exit 1
  fi
  formula_url="https://github.com/quicknode/cli/releases/download/v{{version}}/qn.rb"
  if ! curl -sfL "$formula_url" -o /tmp/qn.rb; then
    echo "Error: could not download $formula_url (does the release exist?)" >&2
    exit 1
  fi
  mkdir -p "{{tap_path}}/Formula"
  cp /tmp/qn.rb "{{tap_path}}/Formula/qn.rb"
  rm /tmp/qn.rb
  cd "{{tap_path}}"
  if git diff --quiet Formula/qn.rb && ! git ls-files --error-unmatch Formula/qn.rb >/dev/null 2>&1; then
    # New file
    git add Formula/qn.rb
  elif git diff --quiet Formula/qn.rb; then
    echo "Formula/qn.rb is already at v{{version}}. Nothing to commit."
    exit 0
  else
    git add Formula/qn.rb
  fi
  git commit -m "qn {{version}}"
  echo
  echo "Committed qn {{version}} to {{tap_path}}. To publish:"
  echo "  git -C {{tap_path}} push"

# Manually update the AUR `qn-bin` package with a PKGBUILD + .SRCINFO
# for a given release. Use this until CI has SSH access to push to the
# AUR git remote — at which point a CI publish-aur job takes over and
# this recipe becomes a manual-recovery fallback.
#
# The PKGBUILD downloads our prebuilt linux-gnu tarballs from the GitHub
# release (separate sha256 per arch). x86_64 and aarch64 only — cargo-dist
# does not build i686 or armv7.
#
# Usage: just release-update-aur-bin 0.1.4 ~/qn/aur-qn-bin
#
# Precondition: aur_path is a clean local clone of
# ssh://aur@aur.archlinux.org/qn-bin.git. First-time setup requires the
# package name `qn-bin` to be registered on the AUR — see AUR docs for
# the initial `git clone` + push.
release-update-aur-bin version aur_path:
  #!/usr/bin/env bash
  set -euo pipefail
  if [[ ! -d "{{aur_path}}/.git" ]]; then
    echo "Error: {{aur_path}} is not a git checkout. Clone ssh://aur@aur.archlinux.org/qn-bin.git there first." >&2
    exit 1
  fi
  if ! git -C "{{aur_path}}" diff --quiet || ! git -C "{{aur_path}}" diff --cached --quiet; then
    echo "Error: {{aur_path}} has uncommitted changes. Commit or stash them first." >&2
    exit 1
  fi
  # Fetch both sha256 sidecars from the release.
  x86_url="https://github.com/quicknode/cli/releases/download/v{{version}}/quicknode-cli-x86_64-unknown-linux-gnu.tar.xz.sha256"
  arm_url="https://github.com/quicknode/cli/releases/download/v{{version}}/quicknode-cli-aarch64-unknown-linux-gnu.tar.xz.sha256"
  if ! x86_line=$(curl -sfL "$x86_url"); then
    echo "Error: could not download $x86_url (does the release exist?)" >&2
    exit 1
  fi
  if ! arm_line=$(curl -sfL "$arm_url"); then
    echo "Error: could not download $arm_url" >&2
    exit 1
  fi
  # Each sidecar is `<hex> *<filename>` — take just the hex.
  x86_hash=$(echo "$x86_line" | awk '{print $1}')
  arm_hash=$(echo "$arm_line" | awk '{print $1}')
  for h in "$x86_hash" "$arm_hash"; do
    if [[ ! "$h" =~ ^[0-9a-f]{64}$ ]]; then
      echo "Error: parsed hash '$h' is not a 64-char hex string." >&2
      exit 1
    fi
  done
  cat > "{{aur_path}}/PKGBUILD" <<EOF
  # Maintainer: Quicknode <support@quicknode.com>
  pkgname=qn-bin
  pkgver={{version}}
  pkgrel=1
  pkgdesc='Command-line interface for the Quicknode SDK'
  arch=('x86_64' 'aarch64')
  url='https://github.com/quicknode/cli'
  license=('MIT')
  depends=('glibc')
  provides=('qn')
  conflicts=('qn')
  source_x86_64=("\$pkgname-\$pkgver-x86_64.tar.xz::https://github.com/quicknode/cli/releases/download/v\$pkgver/quicknode-cli-x86_64-unknown-linux-gnu.tar.xz")
  source_aarch64=("\$pkgname-\$pkgver-aarch64.tar.xz::https://github.com/quicknode/cli/releases/download/v\$pkgver/quicknode-cli-aarch64-unknown-linux-gnu.tar.xz")
  sha256sums_x86_64=('$x86_hash')
  sha256sums_aarch64=('$arm_hash')

  package() {
    local archdir
    case "\$CARCH" in
      x86_64)  archdir='quicknode-cli-x86_64-unknown-linux-gnu' ;;
      aarch64) archdir='quicknode-cli-aarch64-unknown-linux-gnu' ;;
    esac
    install -Dm755 "\$archdir/qn"      "\$pkgdir/usr/bin/qn"
    install -Dm644 "\$archdir/LICENSE" "\$pkgdir/usr/share/licenses/\$pkgname/LICENSE"
    install -Dm644 "\$archdir/README.md" "\$pkgdir/usr/share/doc/\$pkgname/README.md"
  }
  EOF
  # Generate .SRCINFO manually — we can't run `makepkg --printsrcinfo` on
  # macOS dev boxes. Mirrors the deterministic field order makepkg emits.
  cat > "{{aur_path}}/.SRCINFO" <<EOF
  pkgbase = qn-bin
  	pkgdesc = Command-line interface for the Quicknode SDK
  	pkgver = {{version}}
  	pkgrel = 1
  	url = https://github.com/quicknode/cli
  	arch = x86_64
  	arch = aarch64
  	license = MIT
  	depends = glibc
  	provides = qn
  	conflicts = qn
  	source_x86_64 = qn-bin-{{version}}-x86_64.tar.xz::https://github.com/quicknode/cli/releases/download/v{{version}}/quicknode-cli-x86_64-unknown-linux-gnu.tar.xz
  	sha256sums_x86_64 = $x86_hash
  	source_aarch64 = qn-bin-{{version}}-aarch64.tar.xz::https://github.com/quicknode/cli/releases/download/v{{version}}/quicknode-cli-aarch64-unknown-linux-gnu.tar.xz
  	sha256sums_aarch64 = $arm_hash

  pkgname = qn-bin
  EOF
  cd "{{aur_path}}"
  # Stage both, even if only one changed; AUR convention is to commit them together.
  git add PKGBUILD .SRCINFO
  if git diff --cached --quiet PKGBUILD .SRCINFO; then
    echo "PKGBUILD/.SRCINFO already at v{{version}}. Nothing to commit."
    exit 0
  fi
  git commit -m "qn-bin {{version}}"
  echo
  echo "Committed qn-bin {{version}} to {{aur_path}}. To publish:"
  echo "  git -C {{aur_path}} push"

# Manually update the Scoop bucket with a manifest for a given release.
# Use this until CI has a PAT with contents:write on the bucket repo and
# can automate the push — at which point a CI job takes over and this
# recipe becomes a manual-recovery fallback.
#
# The generated manifest includes `checkver` + `autoupdate` so Scoop
# users get new versions on `scoop update` even without us pushing a
# new manifest for every release. We still push to bump the canonical
# `version` field so `scoop search` is honest about what's current.
#
# Usage: just release-update-scoop-bucket 0.1.4 ~/qn/scoop-bucket
#
# Precondition: bucket_path is a clean local clone of quicknode/scoop-bucket.
release-update-scoop-bucket version bucket_path:
  #!/usr/bin/env bash
  set -euo pipefail
  if [[ ! -d "{{bucket_path}}/.git" ]]; then
    echo "Error: {{bucket_path}} is not a git checkout. Clone quicknode/scoop-bucket there first." >&2
    exit 1
  fi
  if ! git -C "{{bucket_path}}" diff --quiet || ! git -C "{{bucket_path}}" diff --cached --quiet; then
    echo "Error: {{bucket_path}} has uncommitted changes. Commit or stash them first." >&2
    exit 1
  fi
  sha_url="https://github.com/quicknode/cli/releases/download/v{{version}}/quicknode-cli-x86_64-pc-windows-msvc.zip.sha256"
  if ! sha_line=$(curl -sfL "$sha_url"); then
    echo "Error: could not download $sha_url (does the release exist?)" >&2
    exit 1
  fi
  # The .sha256 sidecar is `<hex> *<filename>`; take just the hex.
  hash=$(echo "$sha_line" | awk '{print $1}')
  if [[ ! "$hash" =~ ^[0-9a-f]{64}$ ]]; then
    echo "Error: parsed hash '$hash' is not a 64-char hex string." >&2
    exit 1
  fi
  mkdir -p "{{bucket_path}}/bucket"
  cat > "{{bucket_path}}/bucket/qn.json" <<EOF
  {
      "version": "{{version}}",
      "description": "Command-line interface for the Quicknode SDK",
      "homepage": "https://github.com/quicknode/cli",
      "license": "MIT",
      "architecture": {
          "64bit": {
              "url": "https://github.com/quicknode/cli/releases/download/v{{version}}/quicknode-cli-x86_64-pc-windows-msvc.zip",
              "hash": "$hash"
          }
      },
      "bin": "qn.exe",
      "checkver": "github",
      "autoupdate": {
          "architecture": {
              "64bit": {
                  "url": "https://github.com/quicknode/cli/releases/download/v\$version/quicknode-cli-x86_64-pc-windows-msvc.zip"
              }
          }
      }
  }
  EOF
  cd "{{bucket_path}}"
  if git diff --quiet bucket/qn.json && ! git ls-files --error-unmatch bucket/qn.json >/dev/null 2>&1; then
    git add bucket/qn.json
  elif git diff --quiet bucket/qn.json; then
    echo "bucket/qn.json is already at v{{version}}. Nothing to commit."
    exit 0
  else
    git add bucket/qn.json
  fi
  git commit -m "qn {{version}}"
  echo
  echo "Committed qn {{version}} to {{bucket_path}}. To publish:"
  echo "  git -C {{bucket_path}} push"

# Prepend a curated "How to install" section to the GitHub release body for
# v<VERSION>. The block is rendered from packaging/release-notes-install.md.tmpl
# (substituting {{VERSION}}) and bracketed by HTML-comment markers so re-runs
# replace the existing block instead of stacking duplicates. cargo-dist's
# auto-generated body is preserved below a `---` separator.
#
# Usage:
#   just release-update-install-notes 0.1.8            # edits the release
#   just release-update-install-notes 0.1.8 --dry-run  # prints assembled body to stdout
release-update-install-notes version mode="":
  #!/usr/bin/env bash
  set -euo pipefail
  version="{{version}}"
  mode="{{mode}}"
  if [[ -n "$mode" && "$mode" != "--dry-run" ]]; then
    echo "Error: unknown mode '$mode' (expected --dry-run or unset)." >&2
    exit 1
  fi
  tmpl="packaging/release-notes-install.md.tmpl"
  if [[ ! -f "$tmpl" ]]; then
    echo "Error: $tmpl not found." >&2
    exit 1
  fi
  rendered=$(sed "s/{{ '{{' }}VERSION{{ '}}' }}/${version}/g" "$tmpl")
  start_marker='<!-- qn-install-block:start -->'
  end_marker='<!-- qn-install-block:end -->'
  # The block itself does not include the `---` separator — the separator lives
  # between the block and the rest of the body and stays in the body across re-runs.
  block=$(printf '%s\n%s\n%s' "$start_marker" "$rendered" "$end_marker")
  current=$(gh release view "v${version}" --json body --jq .body)
  if [[ "$current" == *"$start_marker"* && "$current" == *"$end_marker"* ]]; then
    # Replace existing block (idempotent re-run). Split body around markers in pure bash.
    before="${current%%$start_marker*}"
    after_with_end="${current#*$start_marker}"
    after="${after_with_end#*$end_marker}"
    new_body="${before}${block}${after}"
  else
    new_body=$(printf '%s\n\n---\n%s' "$block" "$current")
  fi
  if [[ "$mode" == "--dry-run" ]]; then
    printf '%s\n' "$new_body"
    exit 0
  fi
  tmpfile=$(mktemp)
  trap 'rm -f "$tmpfile"' EXIT
  printf '%s\n' "$new_body" > "$tmpfile"
  gh release edit "v${version}" --notes-file "$tmpfile"
  echo "Updated release v${version} with curated install block."

# Run release-update-{homebrew-tap,scoop-bucket,aur-bin} in sequence for
# the latest release tag, then print the three `git push` commands the
# maintainer needs to run to publish. Auto-detects the version from the
# most recent git tag (`v<X.Y.Z>` → `<X.Y.Z>`), or accepts an override.
#
# Expects three sibling clones under `root` (defaults to ~/qn):
#   ~/qn/homebrew-tap   → quicknode/homebrew-tap
#   ~/qn/scoop-bucket   → quicknode/scoop-bucket
#   ~/qn/qn-bin         → ssh://aur@aur.archlinux.org/qn-bin.git
#
# Usage:
#   just release-sync-manual-channels                  # auto-detect version, default root
#   just release-sync-manual-channels ~/work/quicknode # override root
#   just release-sync-manual-channels ~/qn 0.1.4       # override both
release-sync-manual-channels root="~/qn" version="":
  #!/usr/bin/env bash
  set -euo pipefail
  # Expand a leading ~/ to $HOME/ since bash doesn't expand tildes inside
  # variables. Absolute paths pass through unchanged.
  root="{{root}}"
  case "$root" in
    "~")  root="$HOME" ;;
    "~/"*) root="$HOME/${root#\~/}" ;;
  esac
  # If no version was passed, take the latest tag (strip leading v).
  version="{{version}}"
  if [[ -z "$version" ]]; then
    git fetch --tags --quiet
    tag=$(git describe --tags --abbrev=0 2>/dev/null || true)
    if [[ -z "$tag" ]]; then
      echo 'Error: no git tags found and no version override passed. Run: just release-sync-manual-channels ROOT VERSION' >&2
      exit 1
    fi
    version="${tag#v}"
  fi
  echo "Syncing manual channels at v${version} from clones under ${root}/"
  echo
  just release-update-homebrew-tap "$version" "${root}/homebrew-tap"
  echo
  just release-update-scoop-bucket "$version" "${root}/scoop-bucket"
  echo
  just release-update-aur-bin      "$version" "${root}/qn-bin"
  echo
  just release-update-install-notes "$version"
  echo
  echo "Manual channels and release-notes install block updated. To publish, run:"
  echo "  git -C ${root}/homebrew-tap push"
  echo "  git -C ${root}/scoop-bucket push"
  echo "  git -C ${root}/qn-bin push"

# Release Phase 1: bump → branch → PR → merge → tag → GH release → wait for CI.
# Each recipe is callable on its own; release-prepare orchestrates them with prompts.

# Bump version in Cargo.toml on a fresh release/vX.Y.Z branch.
# Usage: just release-bump 0.2.0
release-bump version:
  #!/usr/bin/env bash
  set -euo pipefail
  raw_version="{{version}}"
  if [[ "$raw_version" =~ ^v ]]; then
    echo "Error: version '$raw_version' must not start with 'v'. The 'v' prefix is added automatically when tagging. Try: just release-bump ${raw_version#v}" >&2
    exit 1
  fi
  if [[ ! "$raw_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
    echo "Error: version '$raw_version' is not valid semver (expected X.Y.Z or X.Y.Z-rc.N)." >&2
    exit 1
  fi
  current_branch=$(git rev-parse --abbrev-ref HEAD)
  if [[ "$current_branch" != "main" ]]; then
    echo "Error: must be on main to start a release (currently on '$current_branch')." >&2
    exit 1
  fi
  if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Error: working tree is not clean. Commit or stash changes before bumping." >&2
    exit 1
  fi
  current_version=$(sed -nE 's/^version = "(.+)"/\1/p' Cargo.toml | head -1)
  if [[ "$current_version" == "{{version}}" ]]; then
    echo "Cargo.toml is already at {{version}}. No bump needed — skip to: just release-tag-main {{version}}" >&2
    exit 0
  fi
  git checkout -b "release/v{{version}}"
  sed -i.bak 's/^version = ".*"/version = "{{version}}"/' Cargo.toml && rm Cargo.toml.bak
  # Refresh Cargo.lock so it matches the new version.
  cargo check --quiet
  git add Cargo.toml Cargo.lock
  git commit -m "Release v{{version}}"
  echo "Committed bump on branch release/v{{version}}. Next: just release-open-pr {{version}}"

# Push the release branch and open a PR for the bump commit.
release-open-pr version:
  #!/usr/bin/env bash
  set -euo pipefail
  git push -u origin "release/v{{version}}"
  gh pr create \
    --base main \
    --head "release/v{{version}}" \
    --title "Release v{{version}}" \
    --body "Automated release bump for v{{version}}. Merging this PR triggers the rest of the release flow (tag, GitHub release, CI artifacts)."

# Squash-merge the release PR (with confirmation) and poll until MERGED.
release-merge-pr version:
  #!/usr/bin/env bash
  set -euo pipefail
  pr_state=$(gh pr view "release/v{{version}}" --json state -q .state)
  if [[ "$pr_state" == "MERGED" ]]; then
    echo "PR for release/v{{version}} already merged."
    exit 0
  fi
  read -r -p "Merge release PR for v{{version}} now via 'gh pr merge --squash --delete-branch'? [y/N] " response
  if [[ "$response" =~ ^[Yy]$ ]]; then
    gh pr merge "release/v{{version}}" --squash --delete-branch
  else
    echo "Aborted. Merge the PR manually in GitHub, then re-run: just release-prepare {{version}}" >&2
    exit 1
  fi
  for attempt in $(seq 1 20); do
    pr_state=$(gh pr view "release/v{{version}}" --json state -q .state)
    if [[ "$pr_state" == "MERGED" ]]; then
      echo "PR merged."
      exit 0
    fi
    sleep 3
  done
  echo "Error: PR for release/v{{version}} did not reach MERGED state." >&2
  exit 1

# Tag the post-merge HEAD of main and push the tag. The tag push is what
# triggers .github/workflows/release.yml.
release-tag-main version:
  #!/usr/bin/env bash
  set -euo pipefail
  git checkout main
  git pull --ff-only origin main
  git tag "v{{version}}"
  git push origin "v{{version}}"

# Manual recovery only. Normally cargo-dist's `host` job in release.yml creates
# the GitHub release as part of its upload step; calling this from release-prepare
# would race that job ("release with the same tag name already exists"). Use this
# only when cargo-dist's host job didn't create the release (e.g. CI was disabled
# or failed before the host step).
release-create-tag version:
  gh release create v{{version}} --generate-notes --target main --title "v{{version}}"

# Wait for the release-triggered run of release.yml to finish.
release-wait-ci version:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Waiting for release.yml run for tag v{{version}}..."
  for attempt in $(seq 1 30); do
    run_id=$(gh run list --workflow=release.yml --event=push --limit 20 --json databaseId,headBranch \
      --jq '.[] | select(.headBranch == "v{{version}}") | .databaseId' | head -n1)
    if [[ -n "${run_id:-}" ]]; then
      echo "Found release.yml run $run_id for v{{version}}"
      gh run watch "$run_id" --exit-status
      exit 0
    fi
    echo "  attempt $attempt/30: run not visible yet, sleeping 5s..."
    sleep 5
  done
  echo "Error: timed out waiting for release.yml run for v{{version}} to appear." >&2
  exit 1

# Orchestrates the full Phase 1 release with two confirmation checkpoints:
# one before pushing the bump branch, one before squash-merging the PR.
# Pass yes=1 to skip prompts (for automation).
# Usage: just release-prepare 0.2.0
release-prepare version yes="0":
  #!/usr/bin/env bash
  set -euo pipefail
  if [[ "{{yes}}" != "1" ]]; then
    echo "About to release v{{version}}:"
    echo "  1. Bump version in Cargo.toml"
    echo "  2. Commit on branch release/v{{version}}"
    echo "  --- review diff and confirm before push ---"
    echo "  3. Push branch + open PR (review checkpoint)"
    echo "  4. Merge PR via 'gh pr merge --squash --delete-branch'"
    echo "  5. Tag the merge commit on main and push the tag"
    echo "  6. Wait for release.yml CI (cargo-dist creates the GitHub release + attaches artifacts)"
    echo
    read -r -p "Continue? [y/N] " response
    [[ "$response" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 1; }
  fi
  just release-bump {{version}}
  echo
  echo "=== Bump commit (HEAD) ==="
  git --no-pager show --stat HEAD
  echo
  echo "=== Diff vs main ==="
  git --no-pager diff main...HEAD -- Cargo.toml Cargo.lock
  echo
  if [[ "{{yes}}" != "1" ]]; then
    echo "Review the bump above. Pushing will open a PR for review."
    read -r -p "Push branch release/v{{version}} and open PR? [y/N] " response
    if [[ ! "$response" =~ ^[Yy]$ ]]; then
      echo "Aborted before push. The bump commit exists locally on release/v{{version}} — undo with:"
      echo "  git checkout main && git branch -D release/v{{version}}"
      exit 1
    fi
  fi
  just release-open-pr {{version}}
  just release-merge-pr {{version}}
  just release-tag-main {{version}}
  just release-wait-ci {{version}}
  echo
  echo "Phase 1 complete. Inspect the release at:"
  echo "  https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/v{{version}}"
  echo
  echo "Next: sync the manual channels (Homebrew, Scoop, AUR) by running"
  echo "  just release-sync-manual-channels ~/qn {{version}}"
  echo "(omit the args to auto-detect the version from the latest tag)."
