# Platform checks: type-checking for Linux and Windows from the Mac

The release targets are Linux and Windows, the development machine is a Mac. These two commands type-check the fork for the release targets inside Docker, without touching the host's `target/` or the source tree:

```sh
script/unaligned/check-linux                  # cargo check -p ide, Linux (the container's native target)
script/unaligned/check-windows                # cargo xwin check -p ide, x86_64-pc-windows-msvc
script/unaligned/check-linux -p ui -p workspace
script/unaligned/check-windows -p gpui_windows -p title_bar
```

Each prints cargo's output and ends with one line, `PASS: …` or `FAIL: …`, with the duration and the peak memory. The exit code is 0 on PASS, cargo's exit code (101) on a compile failure, 2 when the check could not be set up (Docker not running, image build failed).

Run them after any change that touches platform-conditional code (`cfg(target_os = …)`, `cfg(windows)`, `cfg(not(target_os = "macos"))`), a crate's dependencies or features, or a signature used by `gpui_linux`, `gpui_windows` or other platform crates. A macOS `cargo check` proves nothing about those.

Run again after the 2026-11-13 rename: `check-linux` and `check-windows` pass for `-p ide` and for `-p cli` (Linux also `-p remote_server`). The rename moved the application to the package `ide` (`crates/ide`), renamed three crates (`app_actions`, `app_env_vars`, `app_credentials_provider`) and every `ZED_…` variable to `IDE_…`, in platform-specific code too.

## Arguments

Everything after the script name goes to `cargo check`. Without `-p`, `--package` or `--workspace` the `ide` crate (the whole application) is checked. Useful extras: `--tests` (also type-check test code), `--message-format short`, `-p cli -p remote_server` (the other shipped binaries; on Windows also `-p auto_update_helper -p explorer_command_injector`).

Environment variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `UNALIGNED_IDE_JOBS` | `6` | cargo's parallelism (`-j`) |
| `UNALIGNED_IDE_MEMORY` | `5g` | memory limit of the container (`docker run --memory`) |

## Durations (measured on the development Mac, 2026-11-12, `-j 6`, when the `ide` package was still called `zed`)

| Run | Linux | Windows |
| --- | --- | --- |
| Building the image (once, and again when `rust-toolchain.toml` or `script/unaligned/docker/` change) | about 3 min, one image for both | |
| First run, empty volumes (`-p ide`, includes fetching every dependency) | 6 min 29 s, peak 2.3 GB | about 3 min (2 min for the seven UI crates, then 1 min 10 s for the rest of the application), peak 4.6 GB |
| Nothing changed | 2 s | 2 s |
| `ui` crate invalidated (91 crates checked again through the incremental cache) | 11 s, peak 1.1 GB | 11 s, peak 1.3 GB |

A real edit costs more than the invalidation measured above (the edited crate is truly re-analysed), but stays far below the first run. Checking one crate alone (`-p workspace`) can take longer than expected the first time (34 s, 2.3 GB measured): cargo resolves features per selection, so some dependencies are checked again with a smaller feature set than under `-p ide`.

## Memory

The Docker VM of the development machine has about 8 GB, of which other containers use about 2 GB. The checks therefore run with `-j 6` and a 5 GB limit on their own container. The limit is there to protect the other containers: without it, a memory spike would make the VM kill an arbitrary process.

- Linux stays around 2.3 GB.
- Windows peaks around 4.6 GB on a first run: four versions of the `windows` crate are checked in parallel and a single one needs a little over 2 GB. Never set `UNALIGNED_IDE_MEMORY` below `3g` for Windows.
- When a compiler is killed for lack of memory, the script says so (`== N compiler process(es) were killed for lack of memory`), keeps what was already checked and runs again by itself with `-j 2`, then `-j 1`. A `FAIL` that mentions `out-of-memory kill(s)` together with `still out of memory` is an environment problem, not a code problem.
- Do not start a first (cold) Linux run and a first Windows run at the same time: each may use up to its 5 GB limit. Run them one after the other (`script/unaligned/check-linux && script/unaligned/check-windows`). Warm runs are small and can overlap.

## How it works

- One image, `unaligned-ide-check` (2.4 GB), built from `script/unaligned/docker/Dockerfile`: Debian 13, the toolchain pinned in `rust-toolchain.toml`, the apt packages of `script/linux`, and for Windows `clang-cl`, `lld-link`, `llvm-rc`, `nasm` and `cargo-xwin` 0.23.1. Nothing is installed on the host.
- The repository is mounted read-only at `/src` and cargo runs with `--locked`: the source tree is never written, so the checks can run while a host build or other agents are working. They see the working tree as it is at that moment, uncommitted changes included.
- Named volumes keep the work between runs:

| Volume | Content | Size after the first full runs |
| --- | --- | --- |
| `unaligned-ide-linux-target` | `CARGO_TARGET_DIR` for Linux | 13 GB |
| `unaligned-ide-windows-target` | `CARGO_TARGET_DIR` for Windows | 19 GB |
| `unaligned-ide-cargo-home` | cargo registry and git checkouts (shared by both) | 2.4 GB |
| `unaligned-ide-xwin-cache` | MSVC CRT and Windows SDK headers and libraries downloaded by cargo-xwin | 1.1 GB |

- Two runs on the same target wait for each other on cargo's lock (`Blocking waiting for file lock`): that is normal.
- The Windows CRT and SDK are downloaded from Microsoft by `xwin` on the first Windows run; using them means accepting Microsoft's licence terms for the Visual Studio Build Tools. They stay in the volume and are not redistributed.

## Reading a failure

- `error[E…]` followed by `--> crates/…`: a real compile error for that target. Paths are relative to the repository. Fix the code, run the same command again.
- `error: the lock file /src/Cargo.lock needs to be updated but --locked was passed`: a `Cargo.toml` changed without `Cargo.lock`. Run any cargo command on the host first (for example `cargo check -p <crate>`), which updates the lock file, then run the check again.
- `killed for lack of memory`: see Memory above.
- `failed to download`, `network failure`, `could not resolve host`: the first run needs the network (crates.io, GitHub, Microsoft's download servers). Run again.
- `FAIL: could not build the unaligned-ide-check image`: the output just above is Docker's; usually the network, or a toolchain version in `rust-toolchain.toml` that rustup cannot find.
- Warnings do not fail the check; they are printed like on the host.

## What the checks do not prove

They are type-checks in the `dev` profile. They do not link, do not run tests, and do not produce an executable.

- Linux: only the container's architecture (aarch64 on this Mac). Code under `cfg(target_arch = "x86_64")` is not seen.
- Windows: only `x86_64-pc-windows-msvc`. Because the host is Linux, the host-side parts of the build scripts that are written with `#[cfg(windows)]` / `#[cfg(target_os = "windows")]` do not run: the Windows resources (icon, manifest, version info through `winresource` in `crates/ide/build.rs`), the ConPTY download, and the HLSL shader compilation with `fxc.exe` in `crates/gpui_windows/build.rs` (release builds only). For the same reason `check-windows --release` is not expected to pass here (not tried): the release code of `gpui_windows` includes `shaders_bytes.rs`, which only its build script on a Windows host generates.
- Code under `cfg(not(debug_assertions))` is not checked on either target.
- Run-time behaviour (window chrome, fonts, key bindings, file dialogs) needs a real Linux or Windows machine, or CI.

## Docker rules (other projects may run in the same Docker)

- Only create or use containers, images and volumes whose names start with `unaligned-ide-`. The scripts follow this: the image `unaligned-ide-check`, the four volumes above, containers named `unaligned-ide-check-<platform>-…` started with `docker run --rm`.
- Never stop, remove, restart, prune or modify any other container, image, volume or network. Never run `docker system prune`, `docker volume prune`, `docker builder prune`, or `docker compose` in another project.
- Do not install anything on the host for these checks: tooling goes in the image.
- To take the disk space back, remove exactly these objects, when no check is running: `docker volume rm unaligned-ide-linux-target unaligned-ide-windows-target unaligned-ide-cargo-home unaligned-ide-xwin-cache` and `docker image rm unaligned-ide-check`. The next run recreates them and is a first run again.
