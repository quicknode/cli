# qn-bin: install the SLSA-attested binary cargo-dist ships, no rebuild.
#
# This spec is built on COPR's mock chroots. `%prep` downloads the per-arch
# linux-gnu tarball from the GitHub Release and verifies it against the
# .sha256 sidecar; `%install` lays the binary + docs into the buildroot.
# No Rust toolchain involved on the COPR side — the binary inside the
# resulting RPM is bit-identical to what ships in crates.io,
# Homebrew, .deb, and the GHCR image.
#
# Built and uploaded by .github/workflows/publish-copr.yml on each release.
# Requires --enable-net=on at build time (mock fetches the tarball).

%global qn_version %{getenv:QN_VERSION}

%if "%{qn_version}" == ""
%{error: QN_VERSION must be set when building this spec (e.g. rpmbuild --define "qn_version 0.1.4")}
%endif

Name:    qn
Version: %{qn_version}
Release: 1%{?dist}
Summary: Command-line interface for the Quicknode SDK
License: MIT
URL:     https://github.com/quicknode/cli

# cargo-dist emits separate tarballs per Rust target triple. We map COPR's
# arch tokens to those triples; the per-arch Source entry below picks the
# right one at build time.
%ifarch x86_64
%global rust_target x86_64-unknown-linux-gnu
%endif
%ifarch aarch64
%global rust_target aarch64-unknown-linux-gnu
%endif

Source0: https://github.com/quicknode/cli/releases/download/v%{version}/quicknode-cli-%{rust_target}.tar.xz
Source1: https://github.com/quicknode/cli/releases/download/v%{version}/quicknode-cli-%{rust_target}.tar.xz.sha256

ExclusiveArch: x86_64 aarch64
BuildRequires: coreutils
BuildRequires: tar
BuildRequires: xz

%description
qn is a command-line interface for Quicknode, built around noun-verb
commands that read naturally for both humans and agents. Manage endpoints,
streams, webhooks, the KV store, teams, usage, and billing.

This package installs the prebuilt binary cargo-dist publishes upstream —
the same SLSA-attested artifact that ships in crates.io, Homebrew, the
GHCR Docker image, the AUR qn-bin package, and Debian .deb files.

%prep
# Verify the tarball matches the sha256 sidecar from the release.
# The sidecar's format is `<hex>  *<filename>`; rewrite the filename to
# point at the local SOURCES path so `sha256sum -c` works.
expected_hash=$(awk '{print $1}' < %{SOURCE1})
actual_hash=$(sha256sum %{SOURCE0} | awk '{print $1}')
if [ "$expected_hash" != "$actual_hash" ]; then
  echo "Error: sha256 mismatch for %{SOURCE0}" >&2
  echo "  expected: $expected_hash" >&2
  echo "  actual:   $actual_hash" >&2
  exit 1
fi
%setup -q -n quicknode-cli-%{rust_target}

%install
install -Dm755 qn         %{buildroot}%{_bindir}/qn
install -Dm644 LICENSE    %{buildroot}%{_licensedir}/%{name}/LICENSE
install -Dm644 README.md  %{buildroot}%{_docdir}/%{name}/README.md

%files
%{_bindir}/qn
%license LICENSE
%doc README.md

%changelog
* Thu Jun 11 2026 Quicknode <support@quicknode.com> - %{version}-1
- Automated build from the GitHub Release upstream.
