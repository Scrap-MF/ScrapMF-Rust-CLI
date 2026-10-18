# Task runner — single source of truth shared by developers and CI.
# Pipeline: fmt → check → clippy → nextest → deny → build --release
# Install missing helpers with: just tools

default:
    @just --list

# Fast gate: fmt + check + clippy + tests via nextest (~40s)
quick:
    #!/usr/bin/env sh
    set -eu
    command -v cargo-nextest >/dev/null 2>&1 || {
        echo "✖ cargo-nextest is required. Install with:" >&2
        echo "    cargo install cargo-nextest --locked" >&2
        exit 1
    }
    echo "[1/4] fmt"
    cargo fmt --check
    echo "[2/4] check (fast-fail on type errors, no codegen)"
    cargo check --all-targets --all-features
    echo "[3/4] clippy"
    cargo clippy --all-targets --all-features -- -D warnings
    echo "[4/4] tests (nextest)"
    cargo nextest run

# Full CI parity: quick + release build + deny + audit (~5 min)
validate: quick
    #!/usr/bin/env sh
    set -eu
    command -v cargo-deny >/dev/null 2>&1 || cargo install cargo-deny --locked
    command -v cargo-audit >/dev/null 2>&1 || cargo install cargo-audit --locked
    echo "[5/8] build release"
    cargo build --release
    echo "[6/7] security policy (deny: advisories+licenses+bans+sources)"
    cargo deny --config .cargo/deny.toml check
    echo "[7/7] all checks passed"

# Escape hatch: standard runner — also runs doc-tests if any are ever added
legacy-test:
    cargo test --all

# Preview the next version suggested by Conventional Commits (no changes made)
release-preview:
    #!/usr/bin/env sh
    set -eu
    echo "→ convco suggests the next version:"
    convco version
    echo ""
    echo "→ release-plz (the tool CI actually runs) suggests:"
    if command -v release-plz >/dev/null 2>&1; then
        # Run against a throwaway clone: `update` rewrites Cargo.toml on
        # success, and release-plz needs an https://github.com/* remote to
        # detect the provider (SSH host aliases like github.com-* break it).
        tmp="$(mktemp -d)"
        git clone --quiet --no-hardlinks . "$tmp"
        git -C "$tmp" remote set-url origin \
            "https://github.com/Scrap-MF/ScrapMF-Rust-CLI.git"
        (cd "$tmp" && RUST_LOG=release_plz=info release-plz update 2>&1 |
            grep -E "next version|up-to-date|ERROR|Caused" || true)
        rm -rf "$tmp"
        echo "→ note: release-plz compares against origin/<default-branch>,"
        echo "  so commits not yet pushed to main are not counted here."
    else
        echo "  release-plz not installed. Install with: just tools"
        echo "  (or grab the musl binary from MarcoIeni/release-plz releases)"
    fi

# One-time helper installation (just, convco, nextest, cargo-deny, cargo-audit)
tools:
    cargo install --locked just convco cargo-nextest cargo-deny
    # release-plz: prebuilt musl binary recommended (cargo build needs rustc
    # newer than this project's MSRV). See MarcoIeni/release-plz releases.
    @echo "optional: install release-plz from https://github.com/MarcoIeni/release-plz/releases"

# Validate GitHub Actions workflows (requires actionlint; optional)
lint-workflows:
    #!/usr/bin/env sh
    if command -v actionlint >/dev/null 2>&1; then
        actionlint .github/workflows/*.yml
    else
        echo "actionlint not installed — skipping (cargo install actionlint)"
    fi

# Test AUR packaging in an Arch container (works on any distro with podman).
# NEVER mounts the repo: copies the PKGBUILD to a temp dir first, so the
# container cannot leave stray files behind. Rootless podman maps container
# root to your host UID, so pacman works as root AND output files stay yours.
# Usage: just pkgtest [VERSION]   # default: 1.1.0 (must be a published tag)
pkgtest version="1.1.0":
    #!/usr/bin/env sh
    set -eu
    command -v podman >/dev/null 2>&1 || {
        echo "✖ podman is required (Fedora: sudo dnf install podman)" >&2
        exit 1
    }
    work="$(mktemp -d /tmp/scrapmf-pkgtest.XXXXXX)"
    sed "s/^pkgver=.*/pkgver={{version}}/" PKGBUILD > "$work/PKGBUILD"
    echo "→ Testing PKGBUILD at pkgver={{version}} in $work"
    if podman run --rm \
        -v "$work:/pkg:Z" docker.io/library/archlinux:base-devel \
        sh -c "
            pacman -Syu --noconfirm base-devel cargo git &&
            useradd -m builder && chown -R builder:builder /pkg &&
            su builder -c 'cd /pkg && makepkg -f' &&
            chown -R 0:0 /pkg"; then
        echo "✔ package built:"
        ls -la "$work"/scrapmf-[0-9]*.pkg.tar.zst
        echo "→ contents:"
        tar -tf "$work"/scrapmf-[0-9]*.pkg.tar.zst | grep -v '^\.\(PKGINFO\|MTREE\|BUILDINFO\)'
        echo "→ kept for inspection: $work  (delete with: rm -rf \"$work\")"
    else
        echo "✖ build failed — workdir kept: $work" >&2
        exit 1
    fi
