#!/usr/bin/env bash
# Shared implementation of script/unaligned/check-linux and check-windows.
# See docs/unaligned/PLATFORM-CHECKS.md.
#
#   check.sh linux|windows [cargo check arguments…]
#
# Docker rules of this project: every image, container and volume created here
# is named `unaligned-ide-…`, and nothing else in Docker is ever touched.

set -euo pipefail

platform="${1:?usage: check.sh linux|windows [cargo check arguments…]}"
shift

docker_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$docker_dir/../../.." && pwd)"

image="unaligned-ide-check"
jobs="${UNALIGNED_IDE_JOBS:-6}"
memory="${UNALIGNED_IDE_MEMORY:-5g}"

rust_version="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$repo_root/rust-toolchain.toml")"
if [ -z "$rust_version" ]; then
    echo "FAIL: cannot read the toolchain channel from rust-toolchain.toml" >&2
    exit 2
fi

if ! docker info >/dev/null 2>&1; then
    echo "FAIL: Docker is not running" >&2
    exit 2
fi

# The image is built once (a few minutes) and rebuilt only when the pinned
# toolchain, the Dockerfile or run-check.sh change: the label records what it
# was built from, so a pruned Docker build cache does not trigger a rebuild.
inputs="$rust_version-$(cat "$docker_dir/Dockerfile" "$docker_dir/run-check.sh" | cksum | tr ' ' '-')"
built_from="$(docker image inspect --format '{{ index .Config.Labels "unaligned-ide.inputs" }}' "$image" 2>/dev/null || true)"
if [ "$built_from" != "$inputs" ]; then
    echo "== building the $image image (Rust $rust_version); this happens once" >&2
    if ! docker build --tag "$image" \
        --build-arg "RUST_VERSION=$rust_version" \
        --label "unaligned-ide.inputs=$inputs" \
        "$docker_dir" >&2; then
        echo "FAIL: could not build the $image image" >&2
        exit 2
    fi
fi

volumes=(
    --volume "$repo_root:/src:ro"
    --volume "unaligned-ide-cargo-home:/cargo-home"
    --volume "unaligned-ide-$platform-target:/target"
)
if [ "$platform" = windows ]; then
    volumes+=(--volume "unaligned-ide-xwin-cache:/xwin-cache")
fi

container="unaligned-ide-check-$platform-$$-$RANDOM"

# The memory limit keeps a runaway compiler from pushing the Docker VM into a
# global out-of-memory kill that could hit the other projects' containers.
exec docker run --rm --init \
    --name "$container" \
    --memory "$memory" \
    --env "CARGO_BUILD_JOBS=$jobs" \
    --env "UNALIGNED_IDE_MEMORY=$memory" \
    --env "CARGO_TARGET_DIR=/target" \
    "${volumes[@]}" \
    "$image" run-check "$platform" "$@"
