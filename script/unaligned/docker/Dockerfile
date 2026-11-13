# Image used by script/unaligned/check-linux and script/unaligned/check-windows.
#
# It only type-checks: it holds the pinned Rust toolchain, the system packages
# listed for Debian in script/linux, and the cross tooling for
# x86_64-pc-windows-msvc (clang-cl, lld-link, llvm-rc, cargo-xwin). The source
# tree is never copied in: the scripts mount it read-only at /src.
#
# Built by docker/check.sh when missing or out of date (the context is this
# directory only, RUST_VERSION is the channel of rust-toolchain.toml).

FROM debian:trixie-slim

ARG RUST_VERSION
ARG CARGO_XWIN_VERSION=0.23.1

ENV DEBIAN_FRONTEND=noninteractive

# First block: the apt list of script/linux (Debian 13 branch), minus the
# runtime daemons (pipewire, xdg-desktop-portal) that a type-check never starts.
# Second block: what the Windows cross-check needs on top (clang-cl and
# lld-link come with clang/lld, llvm-rc and llvm-lib with llvm).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        gcc g++ libasound2-dev libfontconfig-dev libgit2-dev libglib2.0-dev \
        libssl-dev libva-dev libvulkan1 libwayland-dev libx11-xcb-dev \
        libxkbcommon-x11-dev libzstd-dev make cmake clang lld llvm jq git curl \
        gettext-base elfutils libsqlite3-dev musl-tools musl-dev build-essential \
        ca-certificates pkg-config \
        clang-tools nasm ninja-build python3 perl \
    && rm -rf /var/lib/apt/lists/*

# The toolchain lives in the image (/opt); CARGO_HOME is switched to a named
# volume at run time so the registry and git caches survive between runs.
ENV RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

RUN test -n "$RUST_VERSION" \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | CARGO_HOME=/opt/cargo sh -s -- -y --no-modify-path --profile minimal \
            --default-toolchain "$RUST_VERSION" \
            --component clippy \
            --target x86_64-pc-windows-msvc \
    && CARGO_HOME=/opt/cargo cargo install --locked cargo-xwin --version "$CARGO_XWIN_VERSION" \
    && rm -rf /opt/cargo/registry /opt/cargo/git

# RUSTUP_TOOLCHAIN makes rustup ignore /src/rust-toolchain.toml, which would
# otherwise try to download the extra components and targets on every run.
# The scripts pass the same version as RUST_VERSION, so the pin is respected.
ENV RUSTUP_TOOLCHAIN=$RUST_VERSION \
    CARGO_HOME=/cargo-home \
    CARGO_TERM_COLOR=never \
    XWIN_CACHE_DIR=/xwin-cache

# The repository is bind-mounted and owned by the host user: git (used by the
# ide crate's build script for the commit hash) refuses it otherwise.
RUN git config --system --add safe.directory '*'

# Debian only ships the versioned name (clang-cl-19) in /usr/bin; cargo-xwin and
# the cc crate look for plain `clang-cl`.
RUN ln -s "$(ls /usr/lib/llvm-*/bin/clang-cl | sort -V | tail -n 1)" /usr/local/bin/clang-cl \
    && clang-cl --version

COPY run-check.sh /usr/local/bin/run-check
RUN chmod +x /usr/local/bin/run-check

WORKDIR /src
