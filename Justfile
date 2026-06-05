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
# release. Use this until we have a HOMEBREW_TAP_TOKEN secret and can
# add "homebrew" to publish-jobs in dist-workspace.toml — at which
# point the cargo-dist workflow takes over and this recipe becomes
# obsolete.
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
