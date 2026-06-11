# qn-bin: install the SLSA-attested binary cargo-dist ships, no rebuild.
#
# This spec is built on COPR's mock chroots. The SRPM embeds both Linux
# gnu tarballs (x86_64 + aarch64) cargo-dist published for this release;
# the prep section selects the right one for the chroot's arch and the
# install section lays the binary + docs into the buildroot. No Rust
# toolchain involved on the COPR side — the binary inside the resulting
# RPM is bit-identical to what ships in crates.io, Homebrew, .deb, AUR,
# and the GHCR image.
#
# Comments here intentionally avoid the percent-sign character entirely
# — RPM expands macros inside comments at parse time, which corrupts
# the parser state and causes downstream "Name field must be present"
# errors that look unrelated.
#
# Built by .github/workflows/publish-copr.yml on each release. That
# workflow pre-downloads both arch tarballs into ~/rpmbuild/SOURCES/
# before invoking `rpmbuild -bs`, so the resulting SRPM carries the
# sources and COPR's mock can build with --enable-net=off if desired.

Name:    qn
Version: %{qn_version}
Release: 1%{?dist}
Summary: Command-line interface for the Quicknode SDK
License: MIT
URL:     https://github.com/quicknode/cli

# Both arch tarballs ship in the SRPM. The prep section picks one based
# on the chroot's arch.
Source0: https://github.com/quicknode/cli/releases/download/v%{version}/quicknode-cli-x86_64-unknown-linux-gnu.tar.xz
Source1: https://github.com/quicknode/cli/releases/download/v%{version}/quicknode-cli-aarch64-unknown-linux-gnu.tar.xz

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
%ifarch x86_64
%setup -q -T -b 0 -n quicknode-cli-x86_64-unknown-linux-gnu
%endif
%ifarch aarch64
%setup -q -T -b 1 -n quicknode-cli-aarch64-unknown-linux-gnu
%endif

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
