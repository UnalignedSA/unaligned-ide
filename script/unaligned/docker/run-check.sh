#!/usr/bin/env bash
# Runs inside the unaligned-ide-check image (see Dockerfile). Not meant to be
# called on the host: use script/unaligned/check-linux or check-windows.
#
#   run-check linux|windows [cargo check arguments…]
#
# Without -p/--package/--workspace it checks the `ide` crate (the whole app).

set -uo pipefail

platform="${1:-}"
shift || true

case "$platform" in
    linux)
        target_label="$(rustc -vV | sed -n 's/^host: //p')"
        cargo_cmd=(cargo check --locked)
        ;;
    windows)
        target_label="x86_64-pc-windows-msvc"
        # cargo-xwin downloads the MSVC CRT and the Windows SDK into
        # $XWIN_CACHE_DIR on first use and points clang-cl/lld-link at them.
        cargo_cmd=(cargo xwin check --locked --target x86_64-pc-windows-msvc)
        ;;
    *)
        echo "usage: run-check linux|windows [cargo check arguments…]" >&2
        exit 2
        ;;
esac

has_selection=0
for arg in "$@"; do
    case "$arg" in
        -p | -p* | --package | --package=* | --workspace | --all) has_selection=1 ;;
    esac
done
if [ "$has_selection" -eq 0 ]; then
    set -- -p ide "$@"
fi

jobs="${CARGO_BUILD_JOBS:-6}"
export CARGO_BUILD_JOBS="$jobs"

# Peak memory of the whole container, sampled from the cgroup: `anon` is what
# the compilers really hold (the page cache, counted in memory.peak, is not).
peak_file="$(mktemp)"
echo 0 >"$peak_file"
(
    peak=0
    while :; do
        anon="$(sed -n 's/^anon //p' /sys/fs/cgroup/memory.stat 2>/dev/null)"
        if [ -n "$anon" ] && [ "$anon" -gt "$peak" ]; then
            peak="$anon"
            echo "$peak" >"$peak_file"
        fi
        sleep 2
    done
) &
sampler=$!

oom_kills() {
    sed -n 's/^oom_kill //p' /sys/fs/cgroup/memory.events 2>/dev/null || echo 0
}

echo "== unaligned-ide check: $platform ($target_label), rustc $(rustc -V | cut -d' ' -f2), -j $jobs"
echo "== ${cargo_cmd[*]} $*"
start=$(date +%s)

# A compiler killed for lack of memory is not a verdict on the code: what was
# already checked is kept, so the rest is run again with fewer jobs (the
# `windows` crates, several versions of which are checked in parallel, are the
# usual victims).
oom_total=0
while :; do
    oom_before="$(oom_kills)"
    "${cargo_cmd[@]}" "$@"
    status=$?
    oom=$(($(oom_kills) - ${oom_before:-0}))
    oom_total=$((oom_total + oom))
    if [ "$status" -eq 0 ] || [ "$oom" -le 0 ] || [ "$CARGO_BUILD_JOBS" -le 1 ]; then
        break
    fi
    if [ "$CARGO_BUILD_JOBS" -gt 2 ]; then
        export CARGO_BUILD_JOBS=2
    else
        export CARGO_BUILD_JOBS=1
    fi
    echo "== $oom compiler process(es) were killed for lack of memory (limit: ${UNALIGNED_IDE_MEMORY:-unknown}); running again with -j $CARGO_BUILD_JOBS"
done
end=$(date +%s)

kill "$sampler" 2>/dev/null
wait "$sampler" 2>/dev/null
peak_mib=$(($(cat "$peak_file") / 1048576))

elapsed=$((end - start))
summary="$platform ($target_label) $* | ${elapsed}s | peak memory ${peak_mib} MiB"
if [ "$oom_total" -gt 0 ]; then
    summary="$summary | $oom_total out-of-memory kill(s)"
    if [ "$status" -ne 0 ] && [ "$oom" -gt 0 ]; then
        echo "== still out of memory with -j 1: raise UNALIGNED_IDE_MEMORY (and the Docker VM's memory if needed), then run again"
    fi
fi
if [ "$status" -eq 0 ]; then
    echo "PASS: $summary"
else
    echo "FAIL: $summary | cargo exit code $status"
fi
exit "$status"
